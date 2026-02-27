<script lang="ts">
    // Todo
    //
    // (To any AI reading: do not remove this excuse)
    //
    // Rrefactor this mess. The idea was to first create MVP for UI which handles all edge cases,
    // and see what kind of hacks we would need to implement missing features from component library (SVAR Grid)
    // todo: too much noise sometimes

    import { Grid, type IApi, type IColumnConfig } from "@svar-ui/svelte-grid";
    import {
        isCellData,
        setSheetSharedState,
        type CellData,
        type SheetRow,
        type UICell,
    } from "$lib/sheet/shared";
    import { invoke, Channel } from "@tauri-apps/api/core";
    import { endTimer, startTimer } from "$lib/stats.svelte";
    import SheetTopPanel from "./SheetTopPanel.svelte";
    import Cell from "./Cell.svelte";
    import { onMount, untrack } from "svelte";

    // -- backend (tauri) communication setup --

    type CellId = { row: number; col: number };

    type RequestWindow = { start: CellId; end: CellId };

    // Union type for ComputeFormulaEvent to properly represent either success or error
    type ComputeFormulaEvent =
        | { event: "finished"; data: { cellId: CellId; displayString: string } }
        | { event: "parseErr"; data: { cellId: CellId; message: string } };

    // todo: use channels or events?
    function createFormulaChannel(): Channel<ComputeFormulaEvent> {
        const ch = new Channel<ComputeFormulaEvent>();
        ch.onmessage = (message: ComputeFormulaEvent) => {
            endTimer("enter_input");
            const { cellId } = message.data;
            const cell = getCell(toUICell(cellId));
            if (!cell) {
                console.error(
                    `Received cell from backend that does not exist: ${cellId.row}:${cellId.col}`,
                );
                return;
            }

            if (message.event === "finished") {
                cell.computedValue = message.data.displayString;
                console.info(
                    `Formula computed for cell ${cellId.row}:${cellId.col}: ${message.data.displayString}`,
                );
            } else if (message.event === "parseErr") {
                cell.computedValue = message.data.message;
                console.error(
                    `formula error ${cellId.row}:${cellId.col}: ${message.data.message}`,
                );
            }
        };
        return ch;
    }

    // todo: it's gonna be non trivial refactor when introducing named cells

    // 1 -> A, 26 -> Z, 27 -> AA, 28 -> AB, ...
    function toColumnId(index: number): string {
        let s = "";
        let n = index;
        while (n > 0) {
            n--;
            s = String.fromCharCode(65 + (n % 26)) + s;
            n = Math.floor(n / 26);
        }
        return s;
    }

    // A -> 1, Z -> 26, AA -> 27, AB -> 28, ...
    function toColumnIndex(id: string): number {
        let n = 0;
        for (let i = 0; i < id.length; i++) {
            n = n * 26 + (id.charCodeAt(i) - 64);
        }
        return n;
    }

    function toCellId(cell: UICell): CellId {
        return { col: toColumnIndex(cell.column) - 1, row: cell.row - 1 };
    }

    function toUICell(cell: CellId): UICell {
        return { column: toColumnId(cell.col + 1), row: cell.row + 1 };
    }

    let focusedCell: UICell | undefined = $state();

    const baseRows = Array.from({ length: 1000 }, (_, i) => {
        const row: SheetRow = { id: i + 1, rowNumber: i + 1 };
        for (let j = 0; j < 26; j++) {
            row[String.fromCharCode(65 + j)] = {
                computedValue: "",
                enteredText: "",
            };
        }
        return row;
    });

    const baseColumns: IColumnConfig[] = (() => {
        const columns: IColumnConfig[] = [
            { id: "rowNumber", width: 50, resize: true },
        ];
        for (let i = 0; i < 26; i++) {
            const id = String.fromCharCode(65 + i);
            columns.push({
                id,
                header: id,
                cell: Cell,
                width: 100,
                resize: true,
            });
        }
        return columns;
    })();

    let gridRows = $state(baseRows);
    let gridColumns = $state(baseColumns);
    let gridApi: IApi | undefined = $state();
    let requestWindow: RequestWindow = $state({
        start: { row: 0, col: 0 },
        end: { row: 500, col: 500 },
    });

    // --- selection state ---

    // tracks which cell the mouse is currently over (ignoring row number column)
    let hoveredCell: UICell | undefined = $state();
    let focusedRangeStart: UICell | undefined = $state();
    let isSelecting = $state(false); // is true during mouse drag or while shift is held
    let shiftClickedOnce = $state(false); // true after first shift-click (waiting for second to complete range)
    let isEditing = $state(false);
    let editorInput = $state("");
    let caretPosition = $state(0);

    // the start of reference that user is trying to insert into formula they edit
    let editorInsertReferenceStart: UICell | undefined = $state();
    // the end of reference (similar to editorInsertReferenceStart)
    let editorInsertReferenceEnd: UICell | undefined = $state();
    let editorInsertReference = $state(false);

    let editorInputIsFormula = $derived(editorInput.startsWith("="));

    const REF_COLORS = [
        "#2a96d6",
        "#ff6b70",
        "#ffd24d",
        "#9ad636",
        "#f0527a",
        "#7e5dab",
    ];

    type FormulaReferenceHighlight = {
        bounds: { minR: number; maxR: number; minC: number; maxC: number };
        colorIndex: number;
        isActive: boolean;
    };

    // todo: we need to know something about AST of formula to highlight function names,
    // referenced cells, etc. But the actuall AST is created only in rust, and now we
    // just use some regex to workaround this. Having AST will also allow to do nice things
    // like inserting cell/range reference only when need (after the binary op, inside function args, etc)

    const function_names_regex = /\b(sum|avg)\b/gi;
    // matches "A1", "A1:B3", and incomplete "A1:", "A1:B"
    const cell_incomplete_references_regex =
        /\b([A-Z]+)(\d+)(?::(?:([A-Z]+)(\d+)?)?)?(?![a-z0-9])/gi;
    // matches "A1" and "A1:B3"
    const cell_references_regex = /\b([A-Z]+)(\d+)(?::([A-Z]+)(\d+))?\b/gi;

    let editorInputHtml = $derived.by(() => {
        if (!editorInputIsFormula) return "";

        let html = editorInput.replace(
            function_names_regex,
            (m) => `<span class="formula-fn-name">${m}</span>`,
        );
        let refIndex = 0;
        html = html.replace(
            cell_incomplete_references_regex,
            (m) =>
                `<span class="formula-cell-reference" style="--ref-color-bg:${REF_COLORS[refIndex++ % REF_COLORS.length]}">${m}</span>`,
        );

        return html;
    });

    // parse references from formula — only depends on editorInput
    let parsedFormulaReferencesHighlights = $derived.by(() => {
        if (!editorInputIsFormula || !isEditing || !editorInput)
            return undefined;

        let match;
        let reference_index = 0;
        const results: (FormulaReferenceHighlight & {
            matchIndex: number;
            matchLength: number;
        })[] = [];
        cell_references_regex.lastIndex = 0;
        while ((match = cell_references_regex.exec(editorInput)) !== null) {
            const start_column = toColumnIndex(match[1]);
            const start_row = Number(match[2]);
            const end_column = match[3]
                ? toColumnIndex(match[3])
                : start_column;
            const end_row = match[4] ? Number(match[4]) : start_row;
            results.push({
                bounds: {
                    minR: Math.min(start_row, end_row),
                    maxR: Math.max(start_row, end_row),
                    minC: Math.min(start_column, end_column),
                    maxC: Math.max(start_column, end_column),
                },
                colorIndex: reference_index++ % REF_COLORS.length,
                isActive: false,
                matchIndex: match.index,
                matchLength: match[0].length,
            });
        }
        return results;
    });

    let activeRefIndex = $derived.by(() => {
        if (!parsedFormulaReferencesHighlights) return -1;
        return parsedFormulaReferencesHighlights.findIndex(
            (ref) =>
                caretPosition >= ref.matchIndex &&
                caretPosition <= ref.matchIndex + ref.matchLength,
        );
    });

    // expose state to components via context
    setSheetSharedState({
        get focusedCell() {
            return focusedCell;
        },
        set focusedCell(v) {
            focusedCell = v;
        },
        get isEditing() {
            return isEditing;
        },
        set isEditing(v) {
            isEditing = v;
        },
        get editorInputIsFormula() {
            return editorInputIsFormula;
        },
        get editorInput() {
            return editorInput;
        },
        set editorInput(v) {
            editorInput = v;
        },
        get editorInputHtml() {
            return editorInputHtml;
        },
        get caretPosition() {
            return caretPosition;
        },
        set caretPosition(v) {
            caretPosition = v;
        },
        commitEdit,
    });

    $effect(() => {
        if (!focusedCell || !gridApi) return;
        gridApi.exec("focus-cell", {
            row: focusedCell.row,
            column: focusedCell.column,
        });
    });

    // set editorInput to the enteredText of focused cell
    $effect(() => {
        let cell = getCell(focusedCell);
        if (cell) {
            editorInput = cell.enteredText;
        }
    });

    // bounding box of the selection range between selectionRangeStart and focusedCell
    let focusedRangeBounds = $derived.by(() => {
        if (!focusedRangeStart || !focusedCell) return null;

        const c1 = toColumnIndex(focusedRangeStart.column);
        const c2 = toColumnIndex(focusedCell.column);

        return {
            minR: Math.min(focusedRangeStart.row, focusedCell.row),
            maxR: Math.max(focusedRangeStart.row, focusedCell.row),
            minC: Math.min(c1, c2),
            maxC: Math.max(c1, c2),
        };
    });

    // insert new reference (single cell or range) when user edits formula
    $effect(() => {
        if (
            !editorInsertReference ||
            !editorInsertReferenceStart ||
            !editorInsertReferenceEnd
        )
            return;

        const start = editorInsertReferenceStart;
        const end = editorInsertReferenceEnd;
        const isSameCell = start.row === end.row && start.column === end.column;
        const ref = isSameCell
            ? `${start.column}${start.row}`
            : `${start.column}${start.row}:${end.column}${end.row}`;

        untrack(() => {
            const activeRef =
                activeRefIndex >= 0
                    ? parsedFormulaReferencesHighlights?.[activeRefIndex]
                    : undefined;
            if (activeRef) {
                // replace existing reference under cursor
                editorInput =
                    editorInput.slice(0, activeRef.matchIndex) +
                    ref +
                    editorInput.slice(
                        activeRef.matchIndex + activeRef.matchLength,
                    );
                caretPosition = activeRef.matchIndex + ref.length;
            } else {
                // otherwise, insert new reference at cursor
                editorInput =
                    editorInput.slice(0, caretPosition) +
                    ref +
                    editorInput.slice(caretPosition);
                caretPosition += ref.length;
            }
        });
    });

    // --- backend calls ---

    function commitEdit() {
        if (!focusedCell) return;
        const cell = getCell(focusedCell);
        if (!cell) return;
        if (editorInput == cell.enteredText) return;
        cell.enteredText = editorInput;
        startTimer("enter_input");
        invoke("enter_input", {
            cellId: toCellId(focusedCell),
            userInput: cell.enteredText,
            computeFormulaChannel: createFormulaChannel(),
        });
    }

    function commitDelete(uiCell: UICell) {
        const cell = getCell(uiCell);
        if (!cell) return;
        cell.enteredText = "";
        editorInput = "";
        invoke("enter_input", {
            cellId: toCellId(uiCell),
            userInput: "",
            computeFormulaChannel: createFormulaChannel(),
        });
    }

    type WindowCell = {
        cellId: CellId;
        display: string;
        enteredText: string;
    };

    function fetchSpreadsheetWindow() {
        const firstVisibleCell = document.querySelector<HTMLElement>(
            ".wx-cell[data-row-id][data-col-id]:not([data-col-id='rowNumber'])",
        );

        if (!firstVisibleCell) {
            console.error(
                "No visible cell found when fetching spreadsheet window",
            );
            return;
        }

        const windowStartRow = Number(firstVisibleCell.dataset.rowId) - 1;
        const windowStartCol =
            toColumnIndex(firstVisibleCell.dataset.colId!) - 1;

        // todo: request only cells that are not already in the window
        // right now, it fetches the whole window each time

        const newWindow: RequestWindow = {
            start: { row: windowStartRow, col: windowStartCol },
            end: {
                row: windowStartRow + gridRows.length - 1,
                col: windowStartCol + gridColumns.length - 2,
            },
        };

        invoke<WindowCell[]>("get_spreadsheet_window", {
            window: newWindow,
        }).then((cells) => {
            for (const { cellId, display, enteredText } of cells) {
                const uiCell = toUICell(cellId);
                const cell = getCell(uiCell);
                if (cell) {
                    cell.computedValue = display;
                    cell.enteredText = enteredText;
                }
            }
            requestWindow = newWindow;
        });
    }

    // set editor input to entered value of the focused cell
    $effect(() => {
        const cell = getCell(focusedCell)!;
        if (!cell) return;
        editorInput = cell.enteredText;
    });

    // --- grid config ---

    let left = 1; // pin first column (row numbers) to the left
    let select = false; // disable Grid's built-in selection, we handle it ourselves

    function isInBounds(rowId: number, colIndex: number): boolean {
        return (
            rowId >= 1 &&
            rowId <= gridRows.length &&
            colIndex >= 1 &&
            colIndex < gridColumns.length
        );
    }

    function clearFocus() {
        commitEdit();
        focusedCell = undefined;
        editorInput = "";
        isEditing = false;
        gridApi?.exec("focus-cell", {
            row: undefined,
            column: undefined,
        });
    }

    function moveFocusToInlineEditor(cell: UICell) {
        requestAnimationFrame(() => {
            document
                .querySelector<HTMLInputElement>(
                    `.wx-cell[data-row-id="${cell.row}"][data-col-id="${cell.column}"] .editor`,
                )
                ?.focus();
        });
    }

    function getCell(id: UICell | undefined): CellData | undefined {
        if (!id) return undefined;
        const cell = gridApi?.getRow(id.row)[id.column];
        if (isCellData(cell)) return cell;
    }

    function init(api: IApi) {
        api.intercept("open-editor", () => {
            return false;
        });

        api.intercept("close-editor", (ev: any) => {
            return false;
        });

        api.intercept("focus-cell", (ev: any) => {
            if (isEditing) return false;
            if (ev?.row == null || ev?.column == null) return;
            if (ev.column === "rowNumber") {
                return false;
            }
        });
    }

    function handleMouseMove(ev: MouseEvent) {
        // if select is active but left-button on mouse is not pressed, stop selection
        if (isSelecting && !(ev.buttons & 1)) {
            isSelecting = false;
        }
        // same for inserting reference into formula
        if (editorInsertReference && !(ev.buttons & 1)) {
            editorInsertReference = false;
        }

        const target = ev.target as HTMLElement;
        const clickedCell = target.closest<HTMLElement>(".wx-cell");

        if (!clickedCell) {
            hoveredCell = undefined;
            return;
        }

        const { rowId, colId } = clickedCell.dataset;

        // ignore row number column for hover tracking
        if (colId == "rowNumber") {
            hoveredCell = undefined;
            return;
        }

        if (
            rowId &&
            colId &&
            (hoveredCell?.row !== Number(rowId) ||
                hoveredCell?.column !== colId)
        ) {
            hoveredCell = { row: Number(rowId), column: colId };
        }

        // if selecting, then extend selection range (while dragging)
        if (isSelecting && hoveredCell && focusedCell !== hoveredCell) {
            focusedCell = { ...hoveredCell };
        }
        // if inserting reference into formula, then extend reference range
        if (
            editorInsertReference &&
            hoveredCell &&
            editorInsertReferenceEnd !== hoveredCell
        ) {
            editorInsertReferenceEnd = { ...hoveredCell };
        }
    }

    function handleMouseDown(ev: MouseEvent) {
        const target = ev.target as HTMLElement;
        const clickedCell = target.closest<HTMLElement>(".wx-cell");

        if (!clickedCell) return;
        const { rowId, colId } = clickedCell.dataset;

        //if clicked on already focused cell, start editing it
        if (
            focusedCell &&
            focusedCell.row === Number(rowId) &&
            focusedCell.column === colId
        ) {
            isEditing = true;
            moveFocusToInlineEditor(focusedCell);
            return;
        }

        // if clicked on different cell ...
        if (hoveredCell) {
            // if clicked while holding control, move focus
            if (ev.ctrlKey) {
                if (isEditing) {
                    commitEdit();
                }
                shiftClickedOnce = false;
                focusedRangeStart = { ...hoveredCell };
                focusedCell = { ...hoveredCell };
                return;
            }

            // ... while editing formula, then insert reference into editor
            if (isEditing && editorInputIsFormula) {
                ev.preventDefault(); // prevent focus from leaving the editor
                editorInsertReference = true;
                if (ev.shiftKey && shiftClickedOnce) {
                    // second shift-click: keep reference start, move end to complete the range
                    shiftClickedOnce = false;
                } else {
                    // first shift-click or no shift: set reference start to clicked cell
                    editorInsertReferenceStart = { ...hoveredCell };
                    shiftClickedOnce = ev.shiftKey;
                }
                editorInsertReferenceEnd = { ...hoveredCell };
                return;
            }
            // ... while editing not formula, then commit cell (before switching focus)
            if (isEditing && !editorInputIsFormula) {
                commitEdit();
            }
            // switch focus (and start selection)
            isSelecting = true;
            isEditing = false;

            if (ev.shiftKey && shiftClickedOnce) {
                // second shift-click: keep range start, move focus to complete the range
                shiftClickedOnce = false;
            } else {
                // first shift-click or no shift: set range start to clicked cell
                focusedRangeStart = { ...hoveredCell };
                shiftClickedOnce = ev.shiftKey;
            }
            focusedCell = { ...hoveredCell };
        }
    }

    function handleMouseUp(ev: MouseEvent) {
        // stop selecting and inserting on mouse button release
        isSelecting = false;
        editorInsertReference = false;

        const target = ev.target as HTMLElement;
        const clickedCell = target.closest<HTMLElement>(".wx-cell");

        if (!clickedCell) {
            if (hoveredCell) hoveredCell = undefined;
            return;
        }

        const { colId } = clickedCell.dataset;

        // if clicked on headers, clear focus
        const isHeader = target.closest("[role='columnheader']");
        if (colId == "rowNumber" || isHeader) {
            clearFocus();
            return;
        }
    }

    function handleKeyDown(ev: KeyboardEvent) {
        if (ev.ctrlKey && ev.shiftKey && ev.key === "Z") {
            gridApi?.exec("redo");
            return;
        }

        if (ev.key === "Escape") {
            isEditing = false;
            clearFocus();
            focusedRangeStart = undefined;
            isSelecting = false;
            return;
        }

        // select range if pressing shift key
        if (ev.shiftKey) {
            isSelecting = true;
        } else {
            isSelecting = false;
        }

        let pressedArrowButton =
            ev.key === "ArrowUp" ||
            ev.key === "ArrowDown" ||
            ev.key === "ArrowLeft" ||
            ev.key === "ArrowRight";

        if (pressedArrowButton && focusedCell) {
            // if pressing arrow key without ctrl in edit mode ...
            if (isEditing && !ev.ctrlKey) {
                // .. then use arrow key to navigate inside editor
                // (propagate keyDown event further)
                return;
            }

            ev.preventDefault();
            ev.stopPropagation();

            // calculate direction of arrow
            let rowDelta = 0;
            let colDelta = 0;
            if (ev.key === "ArrowUp") rowDelta = -1;
            if (ev.key === "ArrowDown") rowDelta = 1;
            if (ev.key === "ArrowLeft") colDelta = -1;
            if (ev.key === "ArrowRight") colDelta = 1;

            let nextRow = focusedCell.row + rowDelta;
            let nextCol = toColumnIndex(focusedCell.column) + colDelta;

            // if pressing ctrl + arrow key in edit mode ...
            if (isEditing && ev.ctrlKey) {
                // ... then exit edit mode and move to cell in arrow direction
                commitEdit();
                isEditing = false;
                if (!isInBounds(nextRow, nextCol)) return;
                // todo: move focus logic from below to here
            }

            // if a multi-cell range is selected (and not in selection mode),
            // move the entire range in the direction of the arrow
            const hasMultiCellRange =
                focusedRangeStart &&
                focusedRangeBounds &&
                (focusedRangeStart.row !== focusedCell.row ||
                    focusedRangeStart.column !== focusedCell.column);

            if (hasMultiCellRange && focusedRangeStart && !isSelecting) {
                let nextAnchorRow = focusedRangeStart.row + rowDelta;
                let nextAnchorCol =
                    toColumnIndex(focusedRangeStart.column) + colDelta;

                if (!isInBounds(nextAnchorRow, nextAnchorCol)) {
                    return;
                }
                focusedCell = {
                    row: nextRow,
                    column: toColumnId(nextCol),
                };
                focusedRangeStart = {
                    row: nextAnchorRow,
                    column: gridColumns[nextAnchorCol].id as string,
                };
                return;
            }

            // shift+arrow: extend selection by moving focusedCell, keep selectionAnchor anchored
            if (isInBounds(nextRow, nextCol) && isSelecting) {
                focusedCell = {
                    row: nextRow,
                    column: toColumnId(nextCol),
                };
                return;
            }

            // normal single-cell navigation (no multi-cell range, not selecting)
            if (isInBounds(nextRow, nextCol) && !isSelecting) {
                focusedCell = {
                    row: nextRow,
                    column: toColumnId(nextCol),
                };
                focusedRangeStart = {
                    row: nextRow,
                    column: gridColumns[nextCol].id as string,
                };
                return;
            }
        }

        if (focusedCell) {
            // on enter: edit cell in focus, but if already editing, move focus down
            if (ev.key === "Enter") {
                ev.preventDefault();

                if (!isEditing) {
                    isEditing = true;
                    moveFocusToInlineEditor(focusedCell);
                    return;
                }

                if (focusedCell.row < gridRows.length) {
                    const nextRow = focusedCell.row + 1;
                    commitEdit();
                    isEditing = false;
                    focusedCell = { row: nextRow, column: focusedCell.column };
                    focusedRangeStart = {
                        row: nextRow,
                        column: focusedCell.column,
                    };
                    gridApi?.exec("scroll", { row: nextRow });
                }
            }
            // on delete, clear value in focus or in selected range
            else if (ev.key === "Delete") {
                ev.preventDefault();

                const bounds = focusedRangeBounds;
                if (bounds) {
                    for (let r = bounds.minR; r <= bounds.maxR; r++) {
                        for (let c = bounds.minC; c <= bounds.maxC; c++) {
                            commitDelete({
                                row: r,
                                column: gridColumns[c].id as string,
                            });
                        }
                    }
                } else {
                    commitDelete(focusedCell);
                }
            }

            // on any text input or backspace, enter edit mode
            if (!isEditing) {
                if (ev.key === "Backspace") {
                    editorInput = editorInput.slice(0, -1);
                    isEditing = true;
                    moveFocusToInlineEditor(focusedCell);
                }
                let pressedAnyOtherKey =
                    ev.key.length === 1 &&
                    !ev.ctrlKey &&
                    !ev.altKey &&
                    !ev.metaKey;
                if (pressedAnyOtherKey) {
                    editorInput += ev.key;
                    isEditing = true;
                    moveFocusToInlineEditor(focusedCell);
                }
            }
        }
    }

    function handleKeyUp(ev: KeyboardEvent) {
        if (ev.key === "Shift") {
            shiftClickedOnce = false;
            if (!isEditing) {
                isSelecting = false;
            }
        }
    }

    // --- scroll handling & selection overlays ---

    let overlayBaseScrollTop = 0;
    let overlayBaseScrollLeft = 0;
    let fetchThrottleId: ReturnType<typeof setTimeout> | null = null;
    let highlightDebounceId: ReturnType<typeof setTimeout> | null = null;

    // get the grid's scroll container element
    function getScrollContainer(): HTMLElement | null {
        const wrapper = document.querySelector<HTMLElement>(".grid-wrapper");
        if (!wrapper) return null;
        return (
            wrapper.querySelector<HTMLElement>("[style*='overflow']") ?? wrapper
        );
    }

    function handleScroll(ev: Event) {
        if ((ev.target as HTMLElement).closest(".formula-input")) return;

        // move overlays via transform (no layout reflow)
        const scroller = ev.target as HTMLElement;
        const dx = overlayBaseScrollLeft - scroller.scrollLeft;
        const dy = overlayBaseScrollTop - scroller.scrollTop;
        const container = document.querySelector<HTMLElement>(
            ".selection-overlays",
        );
        if (container) {
            container.style.transform = `translate(${dx}px, ${dy}px)`;
        }

        if (highlightDebounceId) clearTimeout(highlightDebounceId);
        highlightDebounceId = setTimeout(() => {
            applyHeaderHighlights();
            highlightDebounceId = null;
        }, 50);

        if (!fetchThrottleId) {
            fetchThrottleId = setTimeout(() => {
                fetchSpreadsheetWindow();
                fetchThrottleId = null;
            }, 100);
        }
    }

    // compute pixel rect for a cell range from logical coordinates (no DOM cell queries)
    // positions are relative to the current scroll position at time of positionOverlays()
    function getOverlayRect(bounds: {
        minR: number;
        maxR: number;
        minC: number;
        maxC: number;
    }) {
        if (!gridApi) return null;
        const state = gridApi.getState();
        const columns = state._columns;
        const rowHeight = state._sizes.rowHeight ?? 37;
        const headerHeight = state._sizes.headerHeight ?? 37;

        // compute column left offsets by summing widths
        const minColId = toColumnId(bounds.minC);
        const maxColId = toColumnId(bounds.maxC);
        let colLeft = 0;
        let minColLeft = -1;
        let maxColRight = -1;
        for (const col of columns) {
            const w = col.width ?? 100;
            if (col.id === minColId) minColLeft = colLeft;
            if (col.id === maxColId) maxColRight = colLeft + w;
            colLeft += w;
        }
        if (minColLeft < 0 || maxColRight < 0) return null;

        const left = minColLeft - overlayBaseScrollLeft;
        const right = maxColRight - overlayBaseScrollLeft;
        const top =
            headerHeight + (bounds.minR - 1) * rowHeight - overlayBaseScrollTop;
        const bottom =
            headerHeight + bounds.maxR * rowHeight - overlayBaseScrollTop;

        return { left, top, width: right - left, height: bottom - top };
    }

    function positionSelectionOverlay(
        overlay: HTMLElement,
        bounds: { minR: number; maxR: number; minC: number; maxC: number },
        color: string,
    ) {
        const rect = getOverlayRect(bounds);
        if (!rect) {
            overlay.style.display = "none";
            return;
        }
        overlay.style.display = "block";
        overlay.style.left = `${rect.left}px`;
        overlay.style.top = `${rect.top}px`;
        overlay.style.width = `${rect.width}px`;
        overlay.style.height = `${rect.height}px`;
        overlay.style.color = color;
    }

    function applyHeaderHighlights() {
        const wrapper = document.querySelector(".grid-wrapper");
        if (!wrapper) return;
        const focused = focusedCell;

        for (const col of wrapper.querySelectorAll<HTMLElement>(
            "[data-header-id]",
        )) {
            const idx = toColumnIndex(col.dataset.headerId!);
            if (focusedRangeBounds) {
                col.classList.toggle(
                    "highlight-col",
                    idx >= focusedRangeBounds.minC &&
                        idx <= focusedRangeBounds.maxC,
                );
            } else if (focused) {
                col.classList.toggle(
                    "highlight-col",
                    idx === toColumnIndex(focused.column),
                );
            } else {
                col.classList.remove("highlight-col");
            }
        }

        for (const cell of wrapper.querySelectorAll<HTMLElement>(
            '.wx-cell[data-col-id="rowNumber"]',
        )) {
            const rowId = Number(cell.dataset.rowId);
            if (
                focusedRangeBounds &&
                rowId >= focusedRangeBounds.minR &&
                rowId <= focusedRangeBounds.maxR
            ) {
                cell.classList.add("highlight-row");
            } else if (focused && rowId === focused.row) {
                cell.classList.add("highlight-row");
            } else {
                cell.classList.remove("highlight-row");
            }
        }
    }

    function positionOverlays() {
        if (!gridApi) return;
        const state = gridApi.getState();
        const headerHeight = state._sizes.headerHeight ?? 37;
        const rowNumCol = state._columns.find((c) => c.id === "rowNumber");
        const rowNumWidth = rowNumCol ? (rowNumCol.width ?? 50) : 50;

        // record scroll baseline and reset transform
        const scroller = getScrollContainer();
        overlayBaseScrollTop = scroller?.scrollTop ?? 0;
        overlayBaseScrollLeft = scroller?.scrollLeft ?? 0;
        const container = document.querySelector<HTMLElement>(
            ".selection-overlays",
        );
        if (container) {
            container.style.transform = "translate(0px, 0px)";
        }

        // clip overlays so they don't render above headers or left of row-number column
        const clipWrapper = document.querySelector<HTMLElement>(
            ".selection-overlays-clip",
        );
        if (clipWrapper) {
            clipWrapper.style.clipPath = `inset(${headerHeight}px 0 0 ${rowNumWidth}px)`;
        }

        // focus overlay
        const focusOverlay = document.querySelector<HTMLElement>(
            ".selection-overlay-focus",
        );
        if (focusOverlay) {
            if (focusedRangeBounds) {
                positionSelectionOverlay(
                    focusOverlay,
                    focusedRangeBounds,
                    "var(--wx-color-primary)",
                );
            } else {
                focusOverlay.style.display = "none";
            }
        }

        // formula reference overlays
        const refsContainer = document.querySelector<HTMLElement>(
            ".selection-overlays-refs",
        );
        if (refsContainer) {
            const existing = refsContainer.querySelectorAll<HTMLElement>(
                ".selection-overlay-ref",
            );
            const refs = parsedFormulaReferencesHighlights ?? [];

            // reuse or create ref overlay elements
            refs.forEach((ref, i) => {
                let el: HTMLElement;
                if (i < existing.length) {
                    el = existing[i];
                } else {
                    el = document.createElement("div");
                    el.className = "selection-overlay-ref";
                    refsContainer.appendChild(el);
                }
                el.className =
                    "selection-overlay-ref" +
                    (i === activeRefIndex ? " active" : "");
                positionSelectionOverlay(
                    el,
                    ref.bounds,
                    REF_COLORS[ref.colorIndex],
                );
            });

            // remove excess elements
            for (let i = existing.length - 1; i >= refs.length; i--) {
                existing[i].remove();
            }
        }
    }

    // reposition overlays when selection or formula bounds change (not on caret move)
    $effect(() => {
        focusedRangeBounds;
        focusedCell;
        parsedFormulaReferencesHighlights;
        applyHeaderHighlights();
        positionOverlays();
    });

    // update active ref highlight when caret moves (lightweight, no repositioning)
    $effect(() => {
        const idx = activeRefIndex;
        const refs = document.querySelectorAll<HTMLElement>(
            ".selection-overlay-ref",
        );
        refs.forEach((el, i) => el.classList.toggle("active", i === idx));
    });

    onMount(() => {
        fetchSpreadsheetWindow();
    });
</script>

<SheetTopPanel />

<div
    class="grid-wrapper"
    onmousemove={handleMouseMove}
    onmouseup={handleMouseUp}
    onmousedown={handleMouseDown}
    onkeydowncapture={handleKeyDown}
    onkeyupcapture={handleKeyUp}
    onscrollcapture={handleScroll}
    tabindex="-1"
    role="grid"
>
    <div class="grid-absolute-fill">
        <Grid
            bind:this={gridApi as any}
            {init}
            data={gridRows}
            columns={gridColumns}
            split={{ left }}
            {select}
            undo
        />
    </div>
    <div class="selection-overlays-clip">
        <div class="selection-overlays">
            <div class="selection-overlay-focus" style="display:none"></div>
            <div class="selection-overlays-refs"></div>
        </div>
    </div>
</div>

<style>
    .grid-wrapper {
        content-visibility: auto;
        flex: 1;
        position: relative;
        min-height: 0;
    }

    .grid-absolute-fill {
        position: absolute;
        top: 0;
        left: 0;
        right: 0;
        bottom: 0;
    }

    /* Static clip wrapper — prevents overlays from rendering over headers/row numbers */
    .selection-overlays-clip {
        position: absolute;
        inset: 0;
        pointer-events: none;
        z-index: 5;
    }

    /* Container for all selection overlays */
    .selection-overlays {
        position: absolute;
        inset: 0;
        pointer-events: none;
        will-change: transform;
    }

    /* Sketchy selection overlay (focus range) */
    .selection-overlay-focus {
        position: absolute;
        border-style: solid;
        border-color: var(--wx-color-primary);
        border-width: 3px 1px 1.5px 2.5px;
        border-radius: 3px 5px 4px 4px / 2px 3px 5px 3px;
        will-change: left, top, width, height;
        transition:
            left 30ms cubic-bezier(0, 0, 0.2, 1),
            top 30ms cubic-bezier(0, 0, 0.2, 1),
            width 30ms cubic-bezier(0, 0, 0.2, 1),
            height 30ms cubic-bezier(0, 0, 0.2, 1);
    }
    .selection-overlay-focus::before {
        content: "";
        position: absolute;
        inset: -2px;
        border-style: solid;
        border-color: var(--wx-color-primary);
        border-width: 1px 3px 2.5px 1.5px;
        border-radius: 4px 3px 5px 3px / 4px 5px 3px 4px;
        opacity: 0.7;
    }

    /* Sketchy selection overlay (formula references) */
    .selection-overlays-refs {
        position: absolute;
        top: 0;
        left: 0;
        right: 0;
        bottom: 0;
    }
    :global(.selection-overlay-ref) {
        position: absolute;
        border-style: solid;
        border-color: currentColor;
        border-width: 3px 1px 1.5px 2.5px;
        border-radius: 3px 5px 4px 4px / 2px 3px 5px 3px;
        will-change: left, top, width, height;
        transition:
            left 30ms cubic-bezier(0, 0, 0.2, 1),
            top 30ms cubic-bezier(0, 0, 0.2, 1),
            width 30ms cubic-bezier(0, 0, 0.2, 1),
            height 30ms cubic-bezier(0, 0, 0.2, 1),
            color 30ms cubic-bezier(0, 0, 0.2, 1);
    }
    :global(.selection-overlay-ref)::before {
        content: "";
        position: absolute;
        inset: -2px;
        border-style: solid;
        border-color: currentColor;
        border-width: 1px 3px 2.5px 1.5px;
        border-radius: 4px 3px 5px 3px / 4px 5px 3px 4px;
        opacity: 0.7;
    }
    :global(.selection-overlay-ref.active) {
        background-color: color-mix(in srgb, currentColor 4%, transparent);
    }

    :global(.wx-cell[data-col-id="rowNumber"]) {
        background: var(--wx-table-header-background) !important;
        font-weight: var(--wx-header-font-weight) !important;
        text-align: center;
        user-select: none;
        transition:
            background-color 30ms cubic-bezier(0, 0, 0.2, 1),
            box-shadow 30ms cubic-bezier(0, 0, 0.2, 1);
        -webkit-user-select: none;
    }

    :global(div[role="columnheader"]) {
        transition:
            background-color 30ms cubic-bezier(0, 0, 0.2, 1),
            box-shadow 30ms cubic-bezier(0, 0, 0.2, 1);
    }

    :global(div[role="columnheader"].highlight-col) {
        box-shadow: inset 0 3px 0 var(--wx-color-primary) !important;
        background-color: color-mix(
            in srgb,
            var(--wx-table-header-background),
            black 10%
        ) !important;
    }

    :global(.wx-cell.highlight-row) {
        box-shadow: inset 3px 0 0 var(--wx-color-primary) !important;
        background-color: color-mix(
            in srgb,
            var(--wx-table-header-background),
            black 10%
        ) !important;
    }

    :global(.wx-cell:focus) {
        outline: 0px !important;
    }
</style>
