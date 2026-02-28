<script lang="ts">
    import { Grid, type IApi, type IColumnConfig } from "@svar-ui/svelte-grid";
    import {
        columnIndexToLetter,
        columnLetterToIndex,
        isCellData,
        setSheetSharedState,
        type CellData,
        type CellId,
        type SheetRow,
        type UICell,
    } from "$lib/sheet/shared";
    import { invoke } from "@tauri-apps/api/core";
    import { emit, listen } from "@tauri-apps/api/event";
    import SheetTopPanel from "./SheetTopPanel.svelte";
    import Cell from "./Cell.svelte";
    import { onMount, untrack } from "svelte";
    import { endTimer, startTimer } from "$lib/stats.svelte";

    // -- backend (tauri) communication setup --

    type RenderCellEvent = {
        cellId: CellId;
        display: string;
        enteredText: string;
        isError: boolean;
    };

    // todo: it's gonna be non trivial refactor when introducing named cells

    /** Convert CellId (0-indexed) to UICell (SVAR grid format). Used at grid boundary only. */
    function toUICell(cell: CellId): UICell {
        return { column: columnIndexToLetter(cell.col), row: cell.row + 1 };
    }

    /** Convert SVAR data attributes (1-indexed row, letter col) to CellId (0-indexed). */
    function domToCellId(rowId: string, colId: string): CellId {
        return { row: Number(rowId) - 1, col: columnLetterToIndex(colId) };
    }

    let focusedCell: CellId | undefined = $state();

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

    // --- selection state ---

    // tracks which cell the mouse is currently over (ignoring row number column)
    let hoveredCell: CellId | undefined = $state();
    let focusedRangeStart: CellId | undefined = $state();
    let isSelecting = $state(false); // is true during mouse drag or while shift is held
    let shiftClickedOnce = $state(false); // true after first shift-click (waiting for second to complete range)
    let isFilling = $state(false);
    let fillOriginalBounds: {
        minR: number;
        maxR: number;
        minC: number;
        maxC: number;
    } | null = null;
    let isEditing = $state(false);
    let editorInput = $state("");
    let caretPosition = $state(0);

    // the start of reference that user is trying to insert into formula they edit
    let editorInsertReferenceStart: CellId | undefined = $state();
    // the end of reference (similar to editorInsertReferenceStart)
    let editorInsertReferenceEnd: CellId | undefined = $state();
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
    // matches "A1", "~A1", "A1:B3", "~A1:B3", and incomplete "A1:", "~A1:B"
    const cell_incomplete_references_regex =
        /~?(?<!\w)([A-Z]+)(\d+)(?::(?:([A-Z]+)(\d+)?)?)?(?![a-z0-9])/gi;
    // matches "A1", "~A1", "A1:B3", "~A1:B3"
    const cell_references_regex =
        /~?(?<!\w)([A-Z]+)(\d+)(?::([A-Z]+)(\d+))?(?!\w)/gi;

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
            const start_col = columnLetterToIndex(match[1]);
            const start_row = Number(match[2]) - 1;
            const end_col = match[3]
                ? columnLetterToIndex(match[3])
                : start_col;
            const end_row = match[4] ? Number(match[4]) - 1 : start_row;
            results.push({
                bounds: {
                    minR: Math.min(start_row, end_row),
                    maxR: Math.max(start_row, end_row),
                    minC: Math.min(start_col, end_col),
                    maxC: Math.max(start_col, end_col),
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
        const ui = toUICell(focusedCell);
        gridApi.exec("focus-cell", {
            row: ui.row,
            column: ui.column,
        });
    });

    // set editorInput to the enteredText of focused cell
    $effect(() => {
        let cell = getCell(focusedCell);
        if (cell) {
            editorInput = cell.enteredText;
        }
    });

    // bounding box of the selection range (0-indexed)
    let focusedRangeBounds = $derived.by(() => {
        if (!focusedRangeStart || !focusedCell) return null;

        return {
            minR: Math.min(focusedRangeStart.row, focusedCell.row),
            maxR: Math.max(focusedRangeStart.row, focusedCell.row),
            minC: Math.min(focusedRangeStart.col, focusedCell.col),
            maxC: Math.max(focusedRangeStart.col, focusedCell.col),
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
        const isSameCell = start.row === end.row && start.col === end.col;
        const startStr = `${columnIndexToLetter(start.col)}${start.row + 1}`;
        const endStr = `${columnIndexToLetter(end.col)}${end.row + 1}`;
        const nonRelativeRef = isSameCell ? startStr : `${startStr}:${endStr}`;

        untrack(() => {
            const activeRef =
                activeRefIndex >= 0
                    ? parsedFormulaReferencesHighlights?.[activeRefIndex]
                    : undefined;
            if (activeRef) {
                // replace existing reference under cursor
                const relative_prefix =
                    editorInput[activeRef.matchIndex] === "~" ? "~" : "";
                const ref = relative_prefix + nonRelativeRef;
                editorInput =
                    editorInput.slice(0, activeRef.matchIndex) +
                    ref +
                    editorInput.slice(
                        activeRef.matchIndex + activeRef.matchLength,
                    );
                caretPosition = activeRef.matchIndex + ref.length;
            } else {
                // otherwise, insert new reference at cursor (but after '=')
                const insertPos = Math.max(caretPosition, 1);
                editorInput =
                    editorInput.slice(0, insertPos) +
                    nonRelativeRef +
                    editorInput.slice(insertPos);
                caretPosition = insertPos + nonRelativeRef.length;
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
        startTimer("render-cell");
        invoke("enter_input", {
            cellId: focusedCell,
            userInput: cell.enteredText,
        });
    }

    function commitDelete(cellId: CellId) {
        const cell = getCell(cellId);
        if (!cell) return;
        cell.enteredText = "";
        editorInput = "";
        startTimer("render-cell");
        invoke("enter_input", { cellId, userInput: "" });
    }

    function commitCellUpdate(cellId: CellId, value: string) {
        const cell = getCell(cellId);
        if (!cell) return;
        cell.enteredText = value;
        startTimer("render-cell");
        invoke("enter_input", { cellId, userInput: value });
    }

    function commitCellFill(
        source: CellId,
        dest: CellId,
        beforeSource?: CellId,
    ) {
        startTimer("render-cell");
        invoke("fill_cell", {
            source,
            dest,
            beforeSource,
        });
    }

    function emitRenderWindowChanged() {
        const firstVisibleCell = document.querySelector<HTMLElement>(
            ".wx-cell[data-row-id][data-col-id]:not([data-col-id='rowNumber'])",
        );

        if (!firstVisibleCell) {
            console.error(
                "No visible cell found when fetching spreadsheet window",
            );
            return;
        }

        const windowStart = domToCellId(
            firstVisibleCell.dataset.rowId!,
            firstVisibleCell.dataset.colId!,
        );

        emit("render-window-changed", {
            start: windowStart,
            end: {
                row: windowStart.row + gridRows.length - 1,
                col: windowStart.col + gridColumns.length - 2,
            },
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

    /** Check if 0-indexed row/col is within the grid. */
    function isInBounds(row: number, col: number): boolean {
        return (
            row >= 0 &&
            row < gridRows.length &&
            col >= 0 &&
            col < gridColumns.length - 1 // -1 for rowNumber column
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

    function moveFocusToInlineEditor(cell: CellId) {
        const ui = toUICell(cell);
        requestAnimationFrame(() => {
            document
                .querySelector<HTMLInputElement>(
                    `.wx-cell[data-row-id="${ui.row}"][data-col-id="${ui.column}"] .editor`,
                )
                ?.focus();
        });
    }

    function getCell(id: CellId | undefined): CellData | undefined {
        if (!id) return undefined;
        const ui = toUICell(id);
        const cell = gridApi?.getRow(ui.row)[ui.column];
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

        if (rowId && colId) {
            const hovered = domToCellId(rowId, colId);
            if (
                hoveredCell?.row !== hovered.row ||
                hoveredCell?.col !== hovered.col
            ) {
                hoveredCell = hovered;
            }
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

    function handleFillStart(ev: MouseEvent) {
        ev.stopPropagation();
        if (!focusedCell) return;
        isFilling = true;
        isSelecting = true;
        fillOriginalBounds = focusedRangeBounds
            ? { ...focusedRangeBounds }
            : null;
    }

    function handleMouseDown(ev: MouseEvent) {
        const target = ev.target as HTMLElement;
        const clickedCell = target.closest<HTMLElement>(".wx-cell");

        if (!clickedCell) return;
        const { rowId, colId } = clickedCell.dataset;

        //if clicked on already focused cell, start editing it
        if (
            focusedCell &&
            rowId &&
            colId &&
            focusedCell.row === Number(rowId) - 1 &&
            focusedCell.col === columnLetterToIndex(colId)
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
        // fill cells on release
        if (isFilling && focusedRangeStart && focusedRangeBounds) {
            const bounds = focusedRangeBounds;
            const orig = fillOriginalBounds ?? bounds;

            // Step 1: fill horizontally (within overlapping row range)
            const hMinR = Math.max(orig.minR, bounds.minR);
            const hMaxR = Math.min(orig.maxR, bounds.maxR);
            if (bounds.maxC > orig.maxC) {
                for (let r = hMinR; r <= hMaxR; r++) {
                    for (let c = orig.maxC + 1; c <= bounds.maxC; c++) {
                        const src: CellId = { row: r, col: c - 1 };
                        const before: CellId | undefined =
                            c - 2 >= orig.minC
                                ? { row: r, col: c - 2 }
                                : undefined;
                        commitCellFill(src, { row: r, col: c }, before);
                    }
                }
            } else if (bounds.minC < orig.minC) {
                for (let r = hMinR; r <= hMaxR; r++) {
                    for (let c = orig.minC - 1; c >= bounds.minC; c--) {
                        const src: CellId = { row: r, col: c + 1 };
                        const before: CellId | undefined =
                            c + 2 <= orig.maxC
                                ? { row: r, col: c + 2 }
                                : undefined;
                        commitCellFill(src, { row: r, col: c }, before);
                    }
                }
            }

            // Step 2: fill vertically (full column range including new columns)
            if (bounds.maxR > orig.maxR) {
                for (let c = bounds.minC; c <= bounds.maxC; c++) {
                    for (let r = orig.maxR + 1; r <= bounds.maxR; r++) {
                        const src: CellId = { row: r - 1, col: c };
                        const before: CellId | undefined =
                            r - 2 >= orig.minR
                                ? { row: r - 2, col: c }
                                : undefined;
                        commitCellFill(src, { row: r, col: c }, before);
                    }
                }
            } else if (bounds.minR < orig.minR) {
                for (let c = bounds.minC; c <= bounds.maxC; c++) {
                    for (let r = orig.minR - 1; r >= bounds.minR; r--) {
                        const src: CellId = { row: r + 1, col: c };
                        const before: CellId | undefined =
                            r + 2 <= orig.maxR
                                ? { row: r + 2, col: c }
                                : undefined;
                        commitCellFill(src, { row: r, col: c }, before);
                    }
                }
            }

            // delete cells that were in original bounds but not in final bounds (shrinking)
            if (fillOriginalBounds) {
                for (let r = orig.minR; r <= orig.maxR; r++) {
                    for (let c = orig.minC; c <= orig.maxC; c++) {
                        if (
                            r >= bounds.minR &&
                            r <= bounds.maxR &&
                            c >= bounds.minC &&
                            c <= bounds.maxC
                        )
                            continue;
                        commitCellUpdate({ row: r, col: c }, "");
                    }
                }
            }

            isFilling = false;
            fillOriginalBounds = null;
        }
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
            let nextCol = focusedCell.col + colDelta;

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
                    focusedRangeStart.col !== focusedCell.col);

            if (hasMultiCellRange && focusedRangeStart && !isSelecting) {
                let nextAnchorRow = focusedRangeStart.row + rowDelta;
                let nextAnchorCol = focusedRangeStart.col + colDelta;

                if (!isInBounds(nextAnchorRow, nextAnchorCol)) {
                    return;
                }
                focusedCell = { row: nextRow, col: nextCol };
                focusedRangeStart = { row: nextAnchorRow, col: nextAnchorCol };
                return;
            }

            // shift+arrow: extend selection by moving focusedCell, keep selectionAnchor anchored
            if (isInBounds(nextRow, nextCol) && isSelecting) {
                focusedCell = { row: nextRow, col: nextCol };
                return;
            }

            // normal single-cell navigation (no multi-cell range, not selecting)
            if (isInBounds(nextRow, nextCol) && !isSelecting) {
                focusedCell = { row: nextRow, col: nextCol };
                focusedRangeStart = { row: nextRow, col: nextCol };
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

                if (focusedCell.row + 1 < gridRows.length) {
                    const nextRow = focusedCell.row + 1;
                    commitEdit();
                    isEditing = false;
                    focusedCell = { row: nextRow, col: focusedCell.col };
                    focusedRangeStart = { row: nextRow, col: focusedCell.col };
                    gridApi?.exec("scroll", { row: nextRow + 1 }); // SVAR expects 1-indexed
                }
            }
            // on delete, clear value in focus or in selected range
            else if (ev.key === "Delete") {
                ev.preventDefault();

                const bounds = focusedRangeBounds;
                if (bounds) {
                    for (let r = bounds.minR; r <= bounds.maxR; r++) {
                        for (let c = bounds.minC; c <= bounds.maxC; c++) {
                            commitDelete({ row: r, col: c });
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
                emitRenderWindowChanged();
                fetchThrottleId = null;
            }, 100);
        }
    }

    // compute pixel rect for a cell range from logical coordinates
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
        const minColLetter = columnIndexToLetter(bounds.minC);
        const maxColLetter = columnIndexToLetter(bounds.maxC);
        let colLeft = 0;
        let minColLeft = -1;
        let maxColRight = -1;
        for (const col of columns) {
            const w = col.width ?? 100;
            if (col.id === minColLetter) minColLeft = colLeft;
            if (col.id === maxColLetter) maxColRight = colLeft + w;
            colLeft += w;
        }
        if (minColLeft < 0 || maxColRight < 0) return null;

        const left = minColLeft - overlayBaseScrollLeft;
        const right = maxColRight - overlayBaseScrollLeft;
        const top =
            headerHeight + bounds.minR * rowHeight - overlayBaseScrollTop;
        const bottom =
            headerHeight + (bounds.maxR + 1) * rowHeight - overlayBaseScrollTop;

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
            const colIdx = columnLetterToIndex(col.dataset.headerId!);
            if (focusedRangeBounds) {
                col.classList.toggle(
                    "highlight-col",
                    colIdx >= focusedRangeBounds.minC &&
                        colIdx <= focusedRangeBounds.maxC,
                );
            } else if (focused) {
                col.classList.toggle("highlight-col", colIdx === focused.col);
            } else {
                col.classList.remove("highlight-col");
            }
        }

        for (const cell of wrapper.querySelectorAll<HTMLElement>(
            '.wx-cell[data-col-id="rowNumber"]',
        )) {
            const rowIdx = Number(cell.dataset.rowId) - 1; // convert to 0-indexed
            if (
                focusedRangeBounds &&
                rowIdx >= focusedRangeBounds.minR &&
                rowIdx <= focusedRangeBounds.maxR
            ) {
                cell.classList.add("highlight-row");
            } else if (focused && rowIdx === focused.row) {
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

        // fill origin overlay (shows original selection during fill)
        const fillOriginOverlay = document.querySelector<HTMLElement>(
            ".selection-overlay-fill-origin",
        );
        if (fillOriginOverlay) {
            if (isFilling && fillOriginalBounds) {
                positionSelectionOverlay(
                    fillOriginOverlay,
                    fillOriginalBounds,
                    "var(--wx-color-primary)",
                );
            } else {
                fillOriginOverlay.style.display = "none";
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
        let unlisten: (() => void) | undefined;

        listen<RenderCellEvent>("render-cell", (event) => {
            endTimer("render-cell");
            const { cellId, display, enteredText } = event.payload;
            const cell = getCell(cellId);
            if (!cell) return;
            cell.computedValue = display;
            cell.enteredText = enteredText;
            // todo: handle isError for styling
        }).then((fn) => {
            unlisten = fn;
            emitRenderWindowChanged();
        });

        return () => {
            unlisten?.();
        };
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
            <div
                class="selection-overlay-fill-origin"
                style="display:none"
            ></div>
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div class="selection-overlay-focus" style="display:none">
                <div
                    class="fill-handle"
                    class:filling={isFilling}
                    onmousedown={handleFillStart}
                ></div>
            </div>
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

    /* Original selection shown during fill drag */
    .selection-overlay-fill-origin {
        position: absolute;
        background: color-mix(in srgb, var(--wx-color-primary) 8%, transparent);
        border: 2px dashed
            color-mix(in srgb, var(--wx-color-primary) 40%, transparent);
        border-radius: 2px;
        pointer-events: none;
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

    .fill-handle {
        position: absolute;
        bottom: -4px;
        right: -4px;
        width: 9px;
        height: 9px;
        background: var(--wx-color-primary);
        border: 1px solid var(--wx-background);
        cursor: crosshair;
        pointer-events: auto;
        z-index: 10;
    }

    .fill-handle.filling {
        width: 12px;
        height: 12px;
        bottom: -6px;
        right: -6px;
        animation: fill-spin 0.8s linear infinite;
    }

    @keyframes fill-spin {
        from {
            transform: rotate(0deg);
        }
        to {
            transform: rotate(360deg);
        }
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
