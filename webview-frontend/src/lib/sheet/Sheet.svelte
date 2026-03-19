<script lang="ts">
    import { Grid, type IApi, type IColumnConfig } from "@svar-ui/svelte-grid";
    import {
        columnIndexToLetter,
        columnLetterToIndex,
        isCellData,
        setSheetSharedState,
        type CellData,
        type CellId,
        type ChangeBounds,
        type SheetRow,
        type UICell,
    } from "$lib/sheet/shared";
    import { invoke } from "@tauri-apps/api/core";
    import SheetFormulaPanel from "./SheetFormulaPanel.svelte";
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
        if (!ev.option) return;
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

    let viewportRowStart = 0;
    let viewportRowEnd = 0;
    let viewportColumnStart = 0;
    let viewportColumnEnd = 25;

    const textDecoder = new TextDecoder();
    const EMPTY_BODY = new Uint8Array();

    /** Decode a single editor value from raw bytes */
    function decodeEditorValue(bytes: Uint8Array): string {
        return textDecoder.decode(bytes);
    }

    /** Decode multiple editor values from backend response: [count: u32][len: u32][bytes]... */
    function decodeEditorValues(bytes: Uint8Array, view: DataView): string[] {
        const values: string[] = [];
        let offset = 0;
        const count = view.getUint32(offset, true);
        offset += 4;
        for (let i = 0; i < count; i++) {
            const len = view.getUint32(offset, true);
            offset += 4;
            values.push(
                textDecoder.decode(bytes.subarray(offset, offset + len)),
            );
            offset += len;
        }
        return values;
    }

    function decodeCells(bytes: Uint8Array, view: DataView) {
        let offset = 0;
        const len = bytes.byteLength;
        while (offset < len) {
            const row = view.getUint32(offset, true);
            const col = view.getUint32(offset + 4, true);
            const isFormula = bytes[offset + 8] !== 0;
            offset += 9;

            const displayLen = view.getUint32(offset, true);
            offset += 4;
            const displayStart = offset;
            offset += displayLen;

            const gridRow = gridApi?.getRow(row + 1);
            if (!gridRow) continue;
            const cell = gridRow[columnIndexToLetter(col)];
            if (!cell || typeof cell !== "object") continue;

            const display = displayLen
                ? textDecoder.decode(
                      bytes.subarray(displayStart, displayStart + displayLen),
                  )
                : "";

            if (cell.computedValue !== display) cell.computedValue = display;
            if (cell.isFormula !== isFormula) cell.isFormula = isFormula;
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

    let focusedCell: CellId | null = $state(null);

    const INITIAL_ROWS = 1000;
    const INITIAL_COLS = 26;
    const COL_WIDTH = 90;

    let rowCount = $state(INITIAL_ROWS);
    let columnCount = $state(INITIAL_COLS);

    function makeRow(i: number, colCount: number): SheetRow {
        const row: SheetRow = { id: i + 1, rowNumber: i + 1 };
        for (let j = 0; j < colCount; j++) {
            row[columnIndexToLetter(j)] = {
                computedValue: "",
                isFormula: false,
            };
        }
        return row;
    }

    const baseRows: SheetRow[] = $state(
        Array.from({ length: INITIAL_ROWS }, (_, i) =>
            makeRow(i, INITIAL_COLS),
        ),
    );

    let gridColumns: IColumnConfig[] = $state(
        (() => {
            const cols: IColumnConfig[] = [
                { id: "rowNumber", width: 50, resize: true },
            ];
            for (let i = 0; i < INITIAL_COLS; i++) {
                const id = columnIndexToLetter(i);
                cols.push({
                    id,
                    header: id,
                    cell: Cell,
                    width: COL_WIDTH,
                    resize: true,
                });
            }
            return cols;
        })(),
    );

    let gridRows: SheetRow[] = $state([]);

    function expandRows(newCount: number) {
        if (newCount <= rowCount) return;
        for (let i = rowCount; i < newCount; i++) {
            baseRows.push(makeRow(i, columnCount));
        }
        rowCount = newCount;
    }

    function expandColumns(newCount: number) {
        if (newCount <= columnCount) return;
        const newCols: IColumnConfig[] = [];
        for (let i = columnCount; i < newCount; i++) {
            const id = columnIndexToLetter(i);
            newCols.push({
                id,
                header: id,
                cell: Cell,
                width: COL_WIDTH,
                resize: true,
            });
            for (const row of baseRows) {
                row[id] = { computedValue: "", isFormula: false };
            }
        }
        gridColumns = [...gridColumns, ...newCols];
        columnCount = newCount;
    }

    /** Ensure columns fill the visible width plus a buffer, and update visible column bounds. */
    function ensureColumnsFillWidth() {
        gridWrapperEl ??= document.querySelector<HTMLElement>(".grid-wrapper");
        if (!gridWrapperEl) return;
        const needed = Math.ceil(gridWrapperEl.clientWidth / COL_WIDTH) + 5;
        if (needed > columnCount) expandColumns(needed);
        updateVisibleColumns();
    }

    function updateVisibleColumns() {
        const scroller = getScrollContainer();
        if (!scroller) return;
        viewportColumnStart = Math.floor(scroller.scrollLeft / COL_WIDTH);
        viewportColumnEnd = Math.min(
            Math.ceil((scroller.scrollLeft + scroller.clientWidth) / COL_WIDTH),
            columnCount - 1,
        );
    }

    let gridApi: IApi | null = $state(null);

    // --- selection state ---

    // tracks which cell the mouse is currently over (ignoring row number column)
    let hoveredCell: CellId | null = $state(null);
    let focusedRangeStart: CellId | null = $state(null);
    let isSelecting = $state(false); // is true during mouse drag or while shift is held
    let shiftClickedOnce = $state(false); // true after first shift-click (waiting for second to complete range)
    let isFilling = $state(false);
    let fillOriginalBounds: CellRange | null = $state(null);
    let clonedFormulaBounds: CellRange | null = $state(null);
    let isEditing = $state(false);
    let editorInput = $state("");
    let caretPosition = $state(0);

    // the start of reference that user is trying to insert into formula they edit
    let editorInsertReferenceStart: CellId | null = $state(null);
    // the end of reference (similar to editorInsertReferenceStart)
    let editorInsertReferenceEnd: CellId | null = $state(null);
    let editorInsertReference = $state(false);

    let editorInputWidth = $state(0);
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
        /(?<!\w)([A-Z]+)(\d+)(?::(?:([A-Z]+)(\d+)?)?)?(?![a-z0-9])/gi;
    // matches "A1", "A1:B3"
    const cell_references_regex =
        /(?<!\w)([A-Z]+)(\d+)(?::([A-Z]+)(\d+))?(?!\w)/gi;

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
        if (!editorInputIsFormula || !isEditing || !editorInput) return null;

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
        get editorInputWidth() {
            return editorInputWidth;
        },
        set editorInputWidth(v) {
            editorInputWidth = v;
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
        const cellRef = isSameCell ? startStr : `${startStr}:${endStr}`;

        untrack(() => {
            const activeRef =
                activeRefIndex >= 0
                    ? parsedFormulaReferencesHighlights?.[activeRefIndex]
                    : null;
            if (activeRef) {
                // replace existing reference under cursor
                editorInput =
                    editorInput.slice(0, activeRef.matchIndex) +
                    cellRef +
                    editorInput.slice(
                        activeRef.matchIndex + activeRef.matchLength,
                    );
                caretPosition = activeRef.matchIndex + cellRef.length;
            } else {
                // otherwise, insert new reference at cursor (but after '=')
                const insertPos = Math.max(caretPosition, 1);
                editorInput =
                    editorInput.slice(0, insertPos) +
                    cellRef +
                    editorInput.slice(insertPos);
                caretPosition = insertPos + cellRef.length;
            }
        });
    });

    // --- backend calls ---

    function commitEdit() {
        if (!focusedCell) return;
        const cell = getCell(focusedCell);
        if (!cell) return;
        if (editorInput == cell.computedValue) return;
        invoke("enter_input", {
            cellId: focusedCell,
            userInput: editorInput,
        });
    }

    function commitDelete(cellIds: CellId[]) {
        if (!cellIds.length) return;
        for (const cellId of cellIds) {
            const cell = getCell(cellId);
            if (!cell) continue;
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
        orig: CellRange,
    ) {
        if (!sources.length || sources.length !== dests.length) return;
        invoke("fill_cells", {
            sources,
            dests,
            origMinRow: orig.minR,
            origMaxRow: orig.maxR,
            origMinCol: orig.minC,
            origMaxCol: orig.maxC,
        });
    }

    async function commitUndo() {
        const bounds: ChangeBounds | null = await invoke("undo_input");
        if (!bounds) return;
        focusedRangeStart = { row: bounds.min_row, col: bounds.min_col };
        focusedCell = { row: bounds.max_row, col: bounds.max_col };
        scrollToRow(bounds.min_row);
    }

    async function commitRedo() {
        const bounds: ChangeBounds | null = await invoke("redo_input");
        if (!bounds) return;
        focusedRangeStart = { row: bounds.min_row, col: bounds.min_col };
        focusedCell = { row: bounds.max_row, col: bounds.max_col };
        scrollToRow(bounds.min_row);
    }

    // --- copy / paste ---

    function copySelection() {
        const cellIds: CellId[] = [];
        let numCols = 1;
        let numRows = 1;

        if (focusedRangeBounds) {
            numRows = focusedRangeBounds.maxR - focusedRangeBounds.minR + 1;
            numCols = focusedRangeBounds.maxC - focusedRangeBounds.minC + 1;
            for (
                let r = focusedRangeBounds.minR;
                r <= focusedRangeBounds.maxR;
                r++
            ) {
                for (
                    let c = focusedRangeBounds.minC;
                    c <= focusedRangeBounds.maxC;
                    c++
                ) {
                    cellIds.push({ row: r, col: c });
                }
            }
        } else if (focusedCell) {
            cellIds.push(focusedCell);
        } else {
            return;
        }

        const hasRange = !!focusedRangeBounds;

        const blobPromise = invoke<ArrayBuffer>("get_editor_value_for_cells", {
            cells: cellIds,
        }).then((response) => {
            const bytes = new Uint8Array(response);
            const values = decodeEditorValues(bytes, new DataView(response));

            let text: string;
            if (hasRange) {
                const rows: string[] = [];
                for (let r = 0; r < numRows; r++) {
                    const cols: string[] = [];
                    for (let c = 0; c < numCols; c++) {
                        cols.push(values[r * numCols + c] ?? "");
                    }
                    rows.push(cols.join("\t"));
                }
                text = rows.join("\n");
            } else {
                text = values[0] ?? "";
            }
            return new Blob([text], { type: "text/plain" });
        });

        navigator.clipboard.write([
            new ClipboardItem({ "text/plain": blobPromise }),
        ]);
    }

    async function pasteFromClipboard() {
        if (!focusedCell) return;
        const text = await navigator.clipboard.readText();
        if (!text) return;

        // parse clipboard as TSV grid
        const clipRows = text.split("\n").map((line) => line.split("\t"));
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
                row: Math.min(startRow + clipRows.length - 1, rowCount - 1),
                col: Math.min(startCol + clipWidth - 1, columnCount - 1),
            };
        }

        if (!pairs.length) return;
        invoke("paste_values", { cells: pairs });
    }

    // --- grid config ---

    // todo: refactor all hardcoded values (like rowHeight, headerHeight, etc) into constants

    let left = 1; // pin first column (row numbers) to the left
    let select = false; // disable Grid's built-in selection, we handle it ourselves
    let sizes = {
        headerHeight: 28,
        rowHeight: 28,
    };

    /** Check if 0-indexed row/col is within the grid, expanding if needed. */
    function isInBounds(row: number, col: number): boolean {
        if (row < 0 || col < 0) return false;
        if (row >= rowCount) expandRows(row + 200);
        if (col >= columnCount) expandColumns(col + 10);
        return true;
    }

    function clearFocus() {
        commitEdit();
        focusedCell = null;
        editorInput = "";
        isEditing = false;
        gridApi?.exec("focus-cell", {
            row: null,
            column: null,
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

    function getCell(id: CellId | null): CellData | null {
        if (!id) return null;
        const ui = toUICell(id);
        const row = gridApi?.getRow(ui.row);
        if (!row) return null;
        const cell = row[ui.column];
        if (isCellData(cell)) return cell;
        return null;
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
            hoveredCell = null;
            return;
        }

        const { rowId, colId } = clickedCell.dataset;

        // ignore row number column for hover tracking
        if (colId == "rowNumber") {
            hoveredCell = null;
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
            // if clicked while holding alt, move focus
            if (ev.altKey) {
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

            const sources: CellId[] = [];
            const dests: CellId[] = [];

            for (let r = bounds.minR; r <= bounds.maxR; r++) {
                for (let c = bounds.minC; c <= bounds.maxC; c++) {
                    if (
                        r >= orig.minR &&
                        r <= orig.maxR &&
                        c >= orig.minC &&
                        c <= orig.maxC
                    )
                        continue;
                    const srcRow = Math.min(Math.max(r, orig.minR), orig.maxR);
                    const srcCol = Math.min(Math.max(c, orig.minC), orig.maxC);
                    sources.push({ row: srcRow, col: srcCol });
                    dests.push({ row: r, col: c });
                }
            }

            if (sources.length) {
                commitCellFill(sources, dests, orig);
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
            if (hoveredCell) hoveredCell = null;
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
            commitRedo();
            return;
        }

        if (ev.ctrlKey && !ev.shiftKey && ev.key === "z") {
            commitUndo();
            return;
        }

        if (ev.ctrlKey && ev.key === "c" && !isEditing) {
            // exit clone mode if pressed ctrl+c
            clonedFormulaBounds = null;

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
                        commitCellFill(sources, dests, src);
                    }
                }
            }
            // otherwise, paste from clipboard
            else {
                pasteFromClipboard();
            }
            return;
        }

        if (ev.ctrlKey && ev.key === "d" && !isEditing) {
            ev.preventDefault();

            // if already cloning, exit clone mode
            if (clonedFormulaBounds) {
                clonedFormulaBounds = null;
                return;
            }

            // otherwise, capture current selection as clone source (enter clone mode)
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
            focusedRangeStart = null;
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
            // if pressing arrow key without alt in edit mode ...
            if (isEditing && !ev.altKey) {
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

            // if pressing alt + arrow key in edit mode ...
            if (isEditing && ev.altKey) {
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

                if (focusedCell.row + 1 < rowCount) {
                    const nextRow = focusedCell.row + 1;
                    commitEdit();
                    isEditing = false;
                    focusedCell = { row: nextRow, col: focusedCell.col };
                    focusedRangeStart = { row: nextRow, col: focusedCell.col };
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

    let sos: SheetObjectsState = $state(null as any);
    let focusOverlay = $state<FocusOverlay>(null as any);
    let fillOriginOverlay = $state<FillOriginOverlay>(null as any);
    let cloneSourceOverlay = $state<CloneSourceOverlay>(null as any);
    let refOverlays: RefOverlay[] = $state([]);
    let overlayDebounceId: ReturnType<typeof setTimeout> | null = null;
    let gridWrapperEl: HTMLElement | null = null;
    let clipWrapperEl: HTMLElement | null = null;
    let overlaysEl: HTMLElement | null = null;
    let scrollContainerEl: HTMLElement | null = null;

    /** Scroll the grid so that the given 0-indexed row is visible. */
    function scrollToRow(row: number) {
        const el = getScrollContainer();
        if (!el) return;
        const top = row * sizes.rowHeight;
        if (top < el.scrollTop) {
            el.scrollTop = top;
        } else if (top + sizes.rowHeight > el.scrollTop + el.clientHeight) {
            el.scrollTop = top + sizes.rowHeight - el.clientHeight;
        }
    }

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

    function moveFocusBackToSpreadsheet() {
        if (document.activeElement !== gridWrapperEl) gridWrapperEl?.focus();
    }

    function initOverlays() {
        sos = createState(clipWrapperEl!, overlaysEl!, gridApi!);
        repositionOverlays();
    }

    function handleScroll(ev: Event) {
        if ((ev.target as HTMLElement).closest(".formula-input")) return;
        const scroller = ev.target as HTMLElement;
        syncScroll(sos, scroller.scrollLeft, scroller.scrollTop);
        updateVisibleColumns();
        if (
            scroller.scrollLeft + scroller.clientWidth >
            scroller.scrollWidth - 200
        ) {
            expandColumns(columnCount + 10);
        }
        if (overlayDebounceId) clearTimeout(overlayDebounceId);
        overlayDebounceId = setTimeout(() => {
            applyHeaderHighlights();
            overlayDebounceId = null;
        }, 10);
        moveFocusBackToSpreadsheet();
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

    export function onFileLoad() {
        // reset grid data
        baseRows.length = 0;
        for (let i = 0; i < INITIAL_ROWS; i++) {
            baseRows.push(makeRow(i, columnCount));
        }

        // reset selection and editing state
        focusedCell = null;
        focusedRangeStart = null;
        hoveredCell = null;
        isSelecting = false;
        shiftClickedOnce = false;
        isFilling = false;
        fillOriginalBounds = null;
        clonedFormulaBounds = null;
        isEditing = false;
        editorInput = "";
        editorInsertReferenceStart = null;
        editorInsertReferenceEnd = null;
        editorInsertReference = false;
        gridRows = baseRows.slice(viewportRowStart, viewportRowEnd + 1);

        // scroll to the top
        const el = getScrollContainer();
        if (el) el.scrollTop = 0;

        ensureColumnsFillWidth();
        restartPolling();
    }

    let pollInterval: ReturnType<typeof setInterval> | undefined;

    function restartPolling() {
        if (pollInterval !== undefined) clearInterval(pollInterval);
        invoke("init_viewport").then(() => {
            pollInterval = setInterval(() => {
                invoke<ArrayBuffer>("get_cells_in_viewport", EMPTY_BODY, {
                    headers: {
                        "row-start": String(viewportRowStart),
                        "row-end": String(viewportRowEnd),
                        "col-start": String(viewportColumnStart),
                        "col-end": String(viewportColumnEnd),
                    },
                }).then((buf) => {
                    // "buf" length is 0 when the cells in the current viewport did not change,
                    // in which case we do nothing
                    if (buf.byteLength > 0) {
                        decodeCells(new Uint8Array(buf), new DataView(buf));
                    }
                });
                // keep editor value fresh for focused cell (skip while user is editing)
                if (focusedCell && !isEditing) {
                    invoke<ArrayBuffer>("get_editor_value_for_cell", {
                        cellId: focusedCell,
                    }).then((response) => {
                        if (!isEditing) {
                            editorInput = decodeEditorValue(
                                new Uint8Array(response),
                            );
                        }
                    });
                }
            }, 16);
        });
    }

    onMount(() => {
        initOverlays();

        // add more columns if window can fit more
        ensureColumnsFillWidth();
        const resizeObs = new ResizeObserver(() => ensureColumnsFillWidth());
        if (gridWrapperEl) resizeObs.observe(gridWrapperEl);

        restartPolling();

        return () => {
            resizeObs.disconnect();
            if (pollInterval !== undefined) clearInterval(pollInterval);
        };
    });

    function handleRequestData(
        ev: { row: { start: number; end: number } } & { [key: string]: any },
    ): void {
        console.log("handle request data");
        const {
            row: { start, end },
        } = ev;
        if (end > rowCount - 100) expandRows(rowCount + 200);
        gridRows = baseRows.slice(start, end + 1);
        viewportRowStart = start;
        viewportRowEnd = end;
    }
</script>

<SheetFormulaPanel />

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
            {sizes}
            {select}
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
                {#each parsedFormulaReferencesHighlights ?? [] as ref, i}
                    <RefOverlay
                        bind:this={refOverlays[i]}
                        {sos}
                        bounds={ref.bounds}
                        color={REF_COLORS[ref.colorIndex]}
                        active={i === activeRefIndex}
                    />
                {/each}
                <FocusOverlay
                    bind:this={focusOverlay}
                    {sos}
                    bounds={focusedRangeBounds}
                    visible={!!focusedRangeBounds}
                    {isFilling}
                    {isEditing}
                    {editorInputWidth}
                    onfillstart={handleFillStart}
                />
            {/if}
        </div>
    </div>
</div>

<style>
    .grid-wrapper {
        flex: 1 1 auto;
        min-height: 0;
        min-width: 0;
        margin-top: 0;
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
        display: flex !important;
        align-items: center;
        justify-content: center;
        user-select: none;
        border-right: var(--wx-table-cell-border) !important;
        transition:
            background-color 30ms cubic-bezier(0, 0, 0.2, 1),
            box-shadow 30ms cubic-bezier(0, 0, 0.2, 1);
        -webkit-user-select: none;
    }

    :global(div[role="columnheader"][data-header-id="rowNumber"]) {
        border-right: var(--wx-table-cell-border) !important;
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
