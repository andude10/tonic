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
    import SheetTopPanel from "./SheetTopPanel.svelte";
    import Cell from "./Cell.svelte";
    import { onMount, untrack } from "svelte";
    import { endTimer, startTimer } from "$lib/stats.svelte";
    import {
        createState,
        syncScroll,
        reposition,
        cellsInBounds,
        cellsOutside,
        type CellRange,
        type SheetObjectsState,
    } from "./overlays/Overlays.svelte";
    import FocusOverlay from "./overlays/FocusOverlay.svelte";
    import FillOriginOverlay from "./overlays/FillOriginOverlay.svelte";
    import CloneSourceOverlay from "./overlays/CloneSourceOverlay.svelte";
    import RefOverlay from "./overlays/RefOverlay.svelte";
    import { ContextMenu, type IMenuOptionClick } from "@svar-ui/svelte-menu";

    const contextMenuOptions = [
        { id: "copy", text: "Copy", icon: "wxi wxi-content-copy" },
        { id: "paste", text: "Paste", icon: "wxi wxi-content-paste" },
    ];

    function handleContextMenuClick(ev: IMenuOptionClick) {
        if (ev.option.id === "copy") copySelection();
        else if (ev.option.id === "paste") pasteFromClipboard();
    }

    function contextMenuResolver(_: any, event: MouseEvent) {
        // before the grid's context menu opens, ...

        const el = (event.target as HTMLElement).closest<HTMLElement>(
            ".wx-cell",
        );
        if (!el) return null;

        // ... ignore if clicked outside of grid (row or column headers)
        const { rowId, colId } = el.dataset;
        if (!rowId || !colId || colId === "rowNumber") return null;
        const clicked = domToCellId(rowId, colId);

        // if right-clicked cell is outside the current selection, move focus there
        const b = focusedRangeBounds;
        const inRange =
            b &&
            clicked.row >= b.minR &&
            clicked.row <= b.maxR &&
            clicked.col >= b.minC &&
            clicked.col <= b.maxC;
        if (!inRange) {
            focusedCell = clicked;
            focusedRangeStart = clicked;
        }
        return clicked;
    }

    // -- backend (tauri) communication setup --

    let viewportStart = 0;
    let viewportEnd = 0;

    const textDecoder = new TextDecoder();
    const EMPTY_BODY = new Uint8Array();

    function decodeCells(bytes: Uint8Array, view: DataView) {
        let offset = 0;
        const len = bytes.byteLength;
        while (offset < len) {
            const row = view.getUint32(offset, true);
            const col = view.getUint32(offset + 4, true);
            // const isError = bytes[offset + 8];
            offset += 9;

            const displayLen = view.getUint32(offset, true);
            offset += 4;
            const displayStart = offset;
            offset += displayLen;

            const enteredLen = view.getUint32(offset, true);
            offset += 4;
            const enteredStart = offset;
            offset += enteredLen;

            const gridRow = gridApi?.getRow(row + 1);
            if (!gridRow) continue;
            const cell = gridRow[columnIndexToLetter(col)];
            if (!cell || typeof cell !== "object") continue;

            const display = displayLen
                ? textDecoder.decode(
                      bytes.subarray(displayStart, displayStart + displayLen),
                  )
                : "";
            const enteredText = enteredLen
                ? textDecoder.decode(
                      bytes.subarray(enteredStart, enteredStart + enteredLen),
                  )
                : "";

            if (cell.computedValue !== display) cell.computedValue = display;
            if (cell.enteredText !== enteredText)
                cell.enteredText = enteredText;
        }
    }

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

    const baseRows: SheetRow[] = $state(
        Array.from({ length: 1000 }, (_, i) => {
            const row: SheetRow = { id: i + 1, rowNumber: i + 1 };
            for (let j = 0; j < 26; j++) {
                row[String.fromCharCode(65 + j)] = {
                    computedValue: "",
                    enteredText: "",
                };
            }
            return row;
        }),
    );

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

    let rowCount = $state(1000);
    let columnCount = $state(26);

    let gridRows: SheetRow[] = $state([]);
    let gridColumns: IColumnConfig[] = $state(baseColumns);
    let gridApi: IApi | undefined = $state();

    // --- selection state ---

    // tracks which cell the mouse is currently over (ignoring row number column)
    let hoveredCell: CellId | undefined = $state();
    let focusedRangeStart: CellId | undefined = $state();
    let isSelecting = $state(false); // is true during mouse drag or while shift is held
    let shiftClickedOnce = $state(false); // true after first shift-click (waiting for second to complete range)
    let isFilling = $state(false);
    let fillOriginalBounds: CellRange | null = $state(null);
    let clonedFormulaBounds: CellRange | null = $state(null);
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
        invoke("enter_input", {
            cellId: focusedCell,
            userInput: cell.enteredText,
        });
    }

    function commitDelete(cellIds: CellId[]) {
        if (!cellIds.length) return;
        for (const cellId of cellIds) {
            const cell = getCell(cellId);
            if (!cell) continue;
            cell.enteredText = "";
            cell.computedValue = "";
            if (
                focusedCell &&
                focusedCell.row === cellId.row &&
                focusedCell.col === cellId.col
            ) {
                editorInput = "";
            }
        }
        invoke("delete_cells", { cells: cellIds });
    }

    function commitCellFill(
        sources: CellId[],
        dests: CellId[],
        beforeSources?: (CellId | undefined)[],
    ) {
        if (!sources.length || sources.length !== dests.length) return;
        invoke("fill_cells", {
            sources,
            dests,
            beforeSources,
        });
    }

    // set editor input to entered value of the focused cell
    $effect(() => {
        const cell = getCell(focusedCell)!;
        if (!cell) return;
        editorInput = cell.enteredText;
    });

    // --- copy / paste ---

    function copySelection() {
        // if focused range, save range as .csv to the clipboard
        if (focusedRangeBounds) {
            const rows: string[] = [];
            for (
                let r = focusedRangeBounds.minR;
                r <= focusedRangeBounds.maxR;
                r++
            ) {
                const cols: string[] = [];
                for (
                    let c = focusedRangeBounds.minC;
                    c <= focusedRangeBounds.maxC;
                    c++
                ) {
                    const cell = getCell({ row: r, col: c });
                    cols.push(cell?.computedValue ?? "");
                }
                rows.push(cols.join(","));
            }
            navigator.clipboard.writeText(rows.join("\n"));
        } else if (focusedCell) {
            // if focused single cell, save computedValue to clipboard
            const cell = getCell(focusedCell);
            navigator.clipboard.writeText(cell?.computedValue ?? "");
        }
    }

    async function pasteFromClipboard() {
        if (!focusedCell) return;
        const text = await navigator.clipboard.readText();
        if (!text) return;

        // parse clipboard as .csv grid
        const clipRows = text.split("\n").map((line) => line.split(","));
        const isSingleClipValue =
            clipRows.length === 1 && clipRows[0].length === 1;
        const hasRange = !!focusedRangeBounds;
        const pairs: [CellId, string][] = [];

        // single value into single focused cell
        if (isSingleClipValue && !hasRange) {
            pairs.push([focusedCell, clipRows[0][0]]);
        }

        // single value into focused range: fill all cells
        if (isSingleClipValue && hasRange) {
            const val = clipRows[0][0];
            for (
                let r = focusedRangeBounds!.minR;
                r <= focusedRangeBounds!.maxR;
                r++
            ) {
                for (
                    let c = focusedRangeBounds!.minC;
                    c <= focusedRangeBounds!.maxC;
                    c++
                ) {
                    pairs.push([{ row: r, col: c }, val]);
                }
            }
        }

        // range into single focused cell or focused range:
        // start from focusedCell (or top-left of range), expand right and down
        if (!isSingleClipValue) {
            const startRow = hasRange
                ? focusedRangeBounds!.minR
                : focusedCell.row;
            const startCol = hasRange
                ? focusedRangeBounds!.minC
                : focusedCell.col;
            const clipWidth = Math.max(...clipRows.map((r) => r.length));
            for (let r = 0; r < clipRows.length; r++) {
                for (let c = 0; c < clipRows[r].length; c++) {
                    const destRow = startRow + r;
                    const destCol = startCol + c;
                    if (!isInBounds(destRow, destCol)) continue;
                    pairs.push([
                        { row: destRow, col: destCol },
                        clipRows[r][c],
                    ]);
                }
            }
            // select the pasted region
            focusedRangeStart = { row: startRow, col: startCol };
            focusedCell = {
                row: Math.min(
                    startRow + clipRows.length - 1,
                    gridRows.length - 1,
                ),
                col: Math.min(startCol + clipWidth - 1, gridColumns.length - 2),
            };
        }

        if (!pairs.length) return;
        invoke("paste_values", { cells: pairs });
    }

    // --- grid config ---

    let left = 1; // pin first column (row numbers) to the left
    let select = false; // disable Grid's built-in selection, we handle it ourselves

    /** Check if 0-indexed row/col is within the grid. */
    function isInBounds(row: number, col: number): boolean {
        return (
            row >= 0 &&
            row < gridRows.length &&
            col >= 0 &&
            col < gridColumns.length - 1
        ); // -1 for rowNumber column;
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
        const row = gridApi?.getRow(ui.row);
        if (!row) return undefined;
        const cell = row[ui.column];
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

        api.on("resize-column", () => {
            requestAnimationFrame(() => repositionOverlays());
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
        // Pin focusedRangeStart to top-left and focusedCell to bottom-right
        // of the current range so that dragging extends it correctly without
        // jumping the selection box.
        if (focusedRangeBounds) {
            focusedRangeStart = {
                row: focusedRangeBounds.minR,
                col: focusedRangeBounds.minC,
            };
            focusedCell = {
                row: focusedRangeBounds.maxR,
                col: focusedRangeBounds.maxC,
            };
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

    /** Generate fill source/dest pairs for one expanded edge of a range.
     *  axis="col" fills horizontally, axis="row" fills vertically.
     *  dir=1 fills forward (right/down), dir=-1 fills backward (left/up). */
    function generateFillEdge(
        orig: CellRange,
        bounds: CellRange,
        axis: "row" | "col",
        dir: 1 | -1,
    ) {
        const sources: CellId[] = [];
        const dests: CellId[] = [];
        const beforeSources: (CellId | undefined)[] = [];

        // cross-axis range: for col fills, iterate rows; for row fills, iterate cols
        const crossMin =
            axis === "col" ? Math.max(orig.minR, bounds.minR) : bounds.minC;
        const crossMax =
            axis === "col" ? Math.min(orig.maxR, bounds.maxR) : bounds.maxC;

        // main-axis range: the expanded edge beyond the original
        const mainStart =
            dir === 1
                ? axis === "col"
                    ? orig.maxC + 1
                    : orig.maxR + 1
                : axis === "col"
                  ? orig.minC - 1
                  : orig.minR - 1;
        const mainEnd =
            dir === 1
                ? axis === "col"
                    ? bounds.maxC
                    : bounds.maxR
                : axis === "col"
                  ? bounds.minC
                  : bounds.minR;

        // limit for "before source" existence check
        const beforeLimit =
            dir === 1
                ? axis === "col"
                    ? orig.minC
                    : orig.minR
                : axis === "col"
                  ? orig.maxC
                  : orig.maxR;

        for (let cross = crossMin; cross <= crossMax; cross++) {
            const start = dir === 1 ? mainStart : mainStart;
            const end = dir === 1 ? mainEnd : mainEnd;
            for (let m = start; dir === 1 ? m <= end : m >= end; m += dir) {
                const makeCell = (v: number) =>
                    axis === "col"
                        ? { row: cross, col: v }
                        : { row: v, col: cross };
                sources.push(makeCell(m - dir));
                dests.push(makeCell(m));
                const beforeVal = m - 2 * dir;
                const hasBefore =
                    dir === 1
                        ? beforeVal >= beforeLimit
                        : beforeVal <= beforeLimit;
                beforeSources.push(hasBefore ? makeCell(beforeVal) : undefined);
            }
        }
        return { sources, dests, beforeSources };
    }

    function handleMouseUp(ev: MouseEvent) {
        // fill cells on release
        if (isFilling && focusedRangeStart && focusedRangeBounds) {
            const bounds = focusedRangeBounds;
            const orig = fillOriginalBounds ?? bounds;

            const allSources: CellId[] = [];
            const allDests: CellId[] = [];
            const allBeforeSources: (CellId | undefined)[] = [];

            const pushEdge = (axis: "row" | "col", dir: 1 | -1) => {
                const { sources, dests, beforeSources } = generateFillEdge(
                    orig,
                    bounds,
                    axis,
                    dir,
                );
                allSources.push(...sources);
                allDests.push(...dests);
                allBeforeSources.push(...beforeSources);
            };

            // horizontal fill (within overlapping row range)
            if (bounds.maxC > orig.maxC) pushEdge("col", 1);
            else if (bounds.minC < orig.minC) pushEdge("col", -1);

            // vertical fill (full column range including new columns)
            if (bounds.maxR > orig.maxR) pushEdge("row", 1);
            else if (bounds.minR < orig.minR) pushEdge("row", -1);

            if (allSources.length) {
                commitCellFill(allSources, allDests, allBeforeSources);
            }

            // delete cells that were in original bounds but not in final bounds (shrinking)
            if (fillOriginalBounds) {
                const deleteCells = cellsOutside(orig, bounds);
                if (deleteCells.length) commitDelete(deleteCells);
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

        if (ev.ctrlKey && ev.key === "c" && !isEditing) {
            ev.preventDefault();
            copySelection();
            return;
        }

        if (ev.ctrlKey && ev.key === "v" && !isEditing) {
            ev.preventDefault();
            // if in cloning mode, clone formula into current selection
            if (clonedFormulaBounds) {
                const src = clonedFormulaBounds;
                const srcHeight = src.maxR - src.minR + 1;
                const srcWidth = src.maxC - src.minC + 1;

                let destBounds: CellRange | null = null;
                if (focusedRangeBounds) {
                    destBounds = { ...focusedRangeBounds };
                } else if (focusedCell) {
                    destBounds = {
                        minR: focusedCell.row,
                        maxR: focusedCell.row,
                        minC: focusedCell.col,
                        maxC: focusedCell.col,
                    };
                }

                if (destBounds) {
                    const sources: CellId[] = [];
                    const dests: CellId[] = [];
                    for (let r = destBounds.minR; r <= destBounds.maxR; r++) {
                        for (
                            let c = destBounds.minC;
                            c <= destBounds.maxC;
                            c++
                        ) {
                            const srcRow =
                                src.minR +
                                ((((r - destBounds.minR) % srcHeight) +
                                    srcHeight) %
                                    srcHeight);
                            const srcCol =
                                src.minC +
                                ((((c - destBounds.minC) % srcWidth) +
                                    srcWidth) %
                                    srcWidth);
                            sources.push({ row: srcRow, col: srcCol });
                            dests.push({ row: r, col: c });
                        }
                    }
                    if (sources.length) {
                        commitCellFill(sources, dests);
                    }
                }
            }
            // otherwise, paste from clipboard
            else {
                pasteFromClipboard();
            }
            return;
        }

        // Ctrl+X: capture current selection as clone source
        if (ev.ctrlKey && ev.key === "x" && !isEditing) {
            ev.preventDefault();
            if (focusedRangeBounds) {
                clonedFormulaBounds = { ...focusedRangeBounds };
            } else if (focusedCell) {
                clonedFormulaBounds = {
                    minR: focusedCell.row,
                    maxR: focusedCell.row,
                    minC: focusedCell.col,
                    maxC: focusedCell.col,
                };
            }
            return;
        }

        if (ev.key === "Escape") {
            isEditing = false;
            clonedFormulaBounds = null;
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

                if (focusedRangeBounds) {
                    commitDelete(cellsInBounds(focusedRangeBounds));
                } else {
                    commitDelete([focusedCell]);
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

    let sos: SheetObjectsState = $state(undefined as any);
    let focusOverlay = $state<FocusOverlay>(undefined as any);
    let fillOriginOverlay = $state<FillOriginOverlay>(undefined as any);
    let cloneSourceOverlay = $state<CloneSourceOverlay>(undefined as any);
    let refOverlays: RefOverlay[] = $state([]);
    let overlayDebounceId: ReturnType<typeof setTimeout> | null = null;
    let gridWrapperEl: HTMLElement | null = null;
    let clipWrapperEl: HTMLElement | null = null;
    let overlaysEl: HTMLElement | null = null;
    let scrollContainerEl: HTMLElement | null = null;

    // get the grid's scroll container element
    function getScrollContainer(): HTMLElement | null {
        if (scrollContainerEl) return scrollContainerEl;
        gridWrapperEl ??= document.querySelector<HTMLElement>(".grid-wrapper");
        if (!gridWrapperEl) return null;
        scrollContainerEl =
            gridWrapperEl.querySelector<HTMLElement>("[style*='overflow']") ??
            gridWrapperEl;
        return scrollContainerEl;
    }

    function initOverlays() {
        sos = createState(clipWrapperEl!, overlaysEl!, gridApi!);
        repositionOverlays();
    }

    function handleScroll(ev: Event) {
        if ((ev.target as HTMLElement).closest(".formula-input")) return;
        const scroller = ev.target as HTMLElement;
        syncScroll(sos, scroller.scrollLeft, scroller.scrollTop);
        if (overlayDebounceId) clearTimeout(overlayDebounceId);
        overlayDebounceId = setTimeout(() => {
            applyHeaderHighlights();
            overlayDebounceId = null;
        }, 10);
        if (document.activeElement !== gridWrapperEl) gridWrapperEl?.focus();
    }

    function applyHeaderHighlights() {
        if (!focusedCell && !focusedRangeBounds) return;
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
            const rowIdx = Number(cell.dataset.rowId) - 1;
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

    function repositionOverlays() {
        if (!gridApi || !sos) return;
        const scroller = getScrollContainer();
        reposition(sos, scroller?.scrollLeft ?? 0, scroller?.scrollTop ?? 0);
        focusOverlay?.reposition();
        fillOriginOverlay?.reposition();
        cloneSourceOverlay?.reposition();
        for (const ref of refOverlays) ref?.reposition();
    }

    // when selection or formula bounds change, reposition overlays and apply header highlight
    $effect(() => {
        focusedRangeBounds;
        focusedCell;
        clonedFormulaBounds;
        parsedFormulaReferencesHighlights;
        applyHeaderHighlights();
        repositionOverlays();
    });

    onMount(() => {
        initOverlays();

        let pollInterval: ReturnType<typeof setInterval>;
        invoke("init_viewport").then(() => {
            pollInterval = setInterval(() => {
                startTimer("poll_cells");
                invoke<ArrayBuffer>("get_cells_in_viewport", EMPTY_BODY, {
                    headers: {
                        "row-start": String(viewportStart),
                        "row-end": String(viewportEnd),
                    },
                }).then((buf) => {
                    // "buf" length is 0 when the cells in the current viewport did not change,
                    // in which case we do nothing
                    if (buf.byteLength > 0) {
                        decodeCells(new Uint8Array(buf), new DataView(buf));
                    }
                    endTimer("poll_cells");
                });
            }, 20);
        });

        return () => {
            clearInterval(pollInterval);
        };
    });

    function handleRequestData(
        ev: { row: { start: number; end: number } } & { [key: string]: any },
    ): void {
        const {
            row: { start, end },
        } = ev;
        gridRows = baseRows.slice(start, end + 1);
        viewportStart = start;
        viewportEnd = end;
    }
</script>

<SheetTopPanel />

<div
    class="grid-wrapper"
    bind:this={gridWrapperEl}
    onmousemove={handleMouseMove}
    onmouseup={handleMouseUp}
    onmousedown={handleMouseDown}
    onkeydowncapture={handleKeyDown}
    onkeyupcapture={handleKeyUp}
    onscrollcapture={handleScroll}
    tabindex="-1"
    role="grid"
>
    <ContextMenu
        options={contextMenuOptions}
        onclick={handleContextMenuClick}
        at="point"
        resolver={contextMenuResolver}
    >
        <Grid
            bind:this={gridApi as any}
            {init}
            data={gridRows}
            columns={gridColumns}
            dynamic={{ rowCount, columnCount }}
            onrequestdata={handleRequestData}
            split={{ left }}
            {select}
            undo
        />
    </ContextMenu>
    <div class="selection-overlays-clip" bind:this={clipWrapperEl}>
        <div class="selection-overlays" bind:this={overlaysEl}>
            {#if sos}
                <FillOriginOverlay
                    bind:this={fillOriginOverlay}
                    {sos}
                    bounds={fillOriginalBounds}
                    visible={isFilling && !!fillOriginalBounds}
                />
                <CloneSourceOverlay
                    bind:this={cloneSourceOverlay}
                    {sos}
                    bounds={clonedFormulaBounds}
                    visible={!!clonedFormulaBounds}
                />
                <FocusOverlay
                    bind:this={focusOverlay}
                    {sos}
                    bounds={focusedRangeBounds}
                    visible={!!focusedRangeBounds}
                    {isFilling}
                    onfillstart={handleFillStart}
                />
                {#each parsedFormulaReferencesHighlights ?? [] as ref, i}
                    <RefOverlay
                        bind:this={refOverlays[i]}
                        {sos}
                        bounds={ref.bounds}
                        color={REF_COLORS[ref.colorIndex]}
                        active={i === activeRefIndex}
                    />
                {/each}
            {/if}
        </div>
    </div>
</div>

<style>
    .grid-wrapper {
        flex: 1 1 auto;
        min-height: 0;
        min-width: 0;
        margin-top: 0.25rem;
        border-top: var(--wx-border, 1px solid #384047);
        position: relative;
        overflow: hidden;
        outline: none;
    }

    .grid-wrapper > :global(.wx-grid) {
        width: 100%;
        height: 100%;
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
