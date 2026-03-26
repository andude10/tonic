<script lang="ts">
    import { Grid, type IApi, type IColumnConfig } from "@svar-ui/svelte-grid";
    import {
        columnIndexToLetter,
        columnLetterToIndex,
        isCellData,
        parseSvarID,
        setSheetSharedState,
        type CellData,
        type CellId,
        type ChangeBounds,
        type SheetRow,
        type TableData,
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
    import SelectionOverlay from "./overlays/SelectionOverlay.svelte";
    import FocusOverlay from "./overlays/FocusOverlay.svelte";
    import FillOriginOverlay from "./overlays/FillOriginOverlay.svelte";
    import CloneSourceOverlay from "./overlays/CloneSourceOverlay.svelte";
    import RefOverlay from "./overlays/RefOverlay.svelte";
    import TableOverlay from "./overlays/TableOverlay.svelte";
    import { listen } from "@tauri-apps/api/event";
    import { ContextMenu, type IMenuOptionClick } from "@svar-ui/svelte-menu";
    import { initNotice } from "$lib/notice";
    import { showError } from "$lib/notice";
    import { getContext } from "svelte";

    const helpers = getContext<{ showNotice: (msg: any) => void }>(
        "wx-helpers",
    );
    if (helpers) initNotice(helpers.showNotice);

    const contextMenuOptions = [
        { id: "copy", text: "Copy", icon: "wxi wxi-content-copy" },
        {
            id: "copy-values",
            text: "Copy Values",
            icon: "wxi wxi-content-copy",
        },
        { id: "paste", text: "Paste", icon: "wxi wxi-content-paste" },
        { id: "create-table", text: "Create Table" },
    ];

    function handleContextMenuClick(ev: IMenuOptionClick) {
        if (!ev.option) return;
        if (ev.option.id === "copy") copySelection();
        else if (ev.option.id === "copy-values") copySelectionValues();
        else if (ev.option.id === "paste") pasteFromClipboard();
        else if (ev.option.id === "create-table") createTableFromSelection();
    }

    function createTableFromSelection() {
        const b = primarySelection;
        if (!b || b.maxR - b.minR < 1) return; // need at least header + 1 body row
        const tableName = `Table ${tables.length + 1}`;
        invoke<number>("create_table", {
            tableName,
            firstHeader: { row: b.minR, col: b.minC },
            lastHeader: { row: b.minR, col: b.maxC },
            bodyStart: { row: b.minR + 1, col: b.minC },
            bodyEnd: { row: b.maxR, col: b.maxC },
        })
            .then((id) => {
                tables = [
                    ...tables,
                    {
                        id,
                        headerBounds: {
                            minR: b.minR,
                            maxR: b.minR,
                            minC: b.minC,
                            maxC: b.maxC,
                        },
                        bodyBounds: {
                            minR: b.minR + 1,
                            maxR: b.maxR,
                            minC: b.minC,
                            maxC: b.maxC,
                        },
                        title: tableName,
                        hasProjection: false,
                        hiddenRowsCount: 0,
                    },
                ];
                requestAnimationFrame(() => repositionOverlays());
            })
            .catch((e) => showError(String(e)));
    }

    function contextMenuResolver(_: any, event: MouseEvent) {
        // before the grid's context menu opens, ...

        const el = (event.target as HTMLElement).closest<HTMLElement>(
            ".wx-cell",
        );
        if (!el) return null;

        // ... ignore if clicked outside of grid (row or column headers)
        const { rowId, colId } = el.dataset;
        if (!rowId || !colId || parseSvarID(colId) === "rowNumber") return null;
        const clicked = domToCellId(rowId, colId);

        // if right-clicked cell is outside the current selection, move focus there
        const inRange =
            isCellInAnyRange(clicked, selections) ||
            (!!focusedCell &&
                clicked.row === focusedCell.row &&
                clicked.col === focusedCell.col);
        if (!inRange) {
            focusedCell = clicked;
            hoveredCell = clicked;
            selections = [];
        }
        return clicked;
    }

    // -- backend (tauri) communication setup --

    let viewportRowStart = 0;
    let viewportRowEnd = 40;
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
        return {
            row: (parseSvarID(rowId) as number) - 1,
            col: columnLetterToIndex(parseSvarID(colId) as string),
        };
    }

    let focusedCell: CellId | null = $state(null);
    let tables: TableData[] = $state([]);

    // Map of "row,col" -> CSS class name for table cells
    let tableCellStyles = $derived.by(() => {
        const map = new Map<string, string>();
        for (const t of tables) {
            const hRow = t.headerBounds.minR;
            for (let c = t.headerBounds.minC; c <= t.headerBounds.maxC; c++) {
                const colId = columnIndexToLetter(c);
                map.set(`${hRow + 1},${colId}`, "table-header-cell");
            }
            for (let r = t.bodyBounds.minR; r <= t.bodyBounds.maxR; r++) {
                const bodyIdx = r - t.bodyBounds.minR;
                const cls =
                    bodyIdx % 2 === 0 ? "table-row-even" : "table-row-odd";
                for (let c = t.bodyBounds.minC; c <= t.bodyBounds.maxC; c++) {
                    map.set(`${r + 1},${columnIndexToLetter(c)}`, cls);
                }
            }
        }
        return map;
    });

    function cellStyle(row: any, col: any): string {
        return tableCellStyles.get(`${row.id},${col.id}`) ?? "";
    }

    const INITIAL_ROWS = 1000;
    const INITIAL_COLS = 26;
    const COL_WIDTH = 90;
    const ROW_NUMBER_WIDTH = 50;

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
                { id: "rowNumber", width: ROW_NUMBER_WIDTH, resize: true },
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

    let addRowsCount = $state(1000);
    let addColsCount = $state(50);
    let showAddRows = $state(false);
    let showAddCols = $state(false);

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
    let selections: CellRange[] = $state([]);
    let isSelecting = $state(false);
    let appendSelectionOnMouseUp = $state(false);
    let isSingleCellSelectionOnMouseUp = $state(false);
    let isFilling = $state(false);
    let fillOriginalBounds: CellRange | null = $state(null);
    let clonedFormulaBounds: CellRange | null = $state(null);
    let isEditing = $state(false);
    let isEditingCellName = $state(false);
    let cellName = $state("");
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
        get isEditingCellName() {
            return isEditingCellName;
        },
        set isEditingCellName(v) {
            isEditingCellName = v;
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
        get cellName() {
            return cellName;
        },
        set cellName(v) {
            cellName = v;
        },
        get tables() {
            return tables;
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

    function cellToRange(cell: CellId): CellRange {
        return {
            minR: cell.row,
            maxR: cell.row,
            minC: cell.col,
            maxC: cell.col,
        };
    }

    function rangeFromCells(a: CellId, b: CellId): CellRange {
        return {
            minR: Math.min(a.row, b.row),
            maxR: Math.max(a.row, b.row),
            minC: Math.min(a.col, b.col),
            maxC: Math.max(a.col, b.col),
        };
    }

    function rangeFromBoundsAndCell(
        bounds: CellRange,
        cell: CellId,
    ): CellRange {
        return {
            minR: Math.min(bounds.minR, cell.row),
            maxR: Math.max(bounds.maxR, cell.row),
            minC: Math.min(bounds.minC, cell.col),
            maxC: Math.max(bounds.maxC, cell.col),
        };
    }

    function isCellInRange(cell: CellId, bounds: CellRange): boolean {
        return (
            cell.row >= bounds.minR &&
            cell.row <= bounds.maxR &&
            cell.col >= bounds.minC &&
            cell.col <= bounds.maxC
        );
    }

    function isCellInAnyRange(cell: CellId, boundsList: CellRange[]): boolean {
        return boundsList.some((bounds) => isCellInRange(cell, bounds));
    }

    function getSelectedCells(): CellId[] {
        const map = new Map<string, CellId>();
        for (const bounds of selections) {
            for (const cell of cellsInBounds(bounds)) {
                map.set(`${cell.row}:${cell.col}`, cell);
            }
        }
        return [...map.values()];
    }

    let primarySelection = $derived.by(() => {
        if (selections.length) return selections[selections.length - 1];
        if (focusedCell) return cellToRange(focusedCell);
        return null;
    });

    let focusedCellBounds = $derived(
        focusedCell ? cellToRange(focusedCell) : null,
    );

    function isSingleCellRange(bounds: CellRange | null): boolean {
        return (
            !!bounds &&
            bounds.minR === bounds.maxR &&
            bounds.minC === bounds.maxC
        );
    }

    let activeSelectionBounds = $derived.by(() => {
        if (isFilling && fillOriginalBounds && hoveredCell) {
            return rangeFromBoundsAndCell(fillOriginalBounds, hoveredCell);
        }
        if (focusedCell && isSelecting && hoveredCell) {
            return rangeFromCells(focusedCell, hoveredCell);
        }
        if (selections.length) {
            return selections[selections.length - 1];
        }
        return focusedCellBounds;
    });

    let activeFocusHasBorder = $derived.by(() => {
        if (!activeSelectionBounds) return false;
        if (isFilling) return true;
        if (isSelecting) return isSingleCellRange(activeSelectionBounds);
        return true;
    });

    let activeFocusHasBackground = $derived.by(() => {
        if (!activeSelectionBounds) return false;
        if (isFilling) return false;
        if (isSelecting) return !isSingleCellRange(activeSelectionBounds);
        return !isSingleCellRange(activeSelectionBounds);
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
        focusedCell = { row: bounds.max_row, col: bounds.max_col };
        hoveredCell = { ...focusedCell };
        selections = [
            {
                minR: bounds.min_row,
                maxR: bounds.max_row,
                minC: bounds.min_col,
                maxC: bounds.max_col,
            },
        ];
        scrollToRow(bounds.min_row);
    }

    async function commitRedo() {
        const bounds: ChangeBounds | null = await invoke("redo_input");
        if (!bounds) return;
        focusedCell = { row: bounds.max_row, col: bounds.max_col };
        hoveredCell = { ...focusedCell };
        selections = [
            {
                minR: bounds.min_row,
                maxR: bounds.max_row,
                minC: bounds.min_col,
                maxC: bounds.max_col,
            },
        ];
        scrollToRow(bounds.min_row);
    }

    // --- copy / paste ---

    function copySelection() {
        const bounds = primarySelection;
        if (!bounds) return;

        const cellIds = cellsInBounds(bounds);
        const numRows = bounds.maxR - bounds.minR + 1;
        const numCols = bounds.maxC - bounds.minC + 1;
        const hasRange = numRows > 1 || numCols > 1;

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

    function copySelectionValues() {
        const bounds = primarySelection;
        if (!bounds) return;

        const cells = cellsInBounds(bounds);
        const numRows = bounds.maxR - bounds.minR + 1;
        const numCols = bounds.maxC - bounds.minC + 1;
        const hasRange = numRows > 1 || numCols > 1;
        let text: string;
        if (hasRange) {
            const rows: string[] = [];
            for (let r = 0; r < numRows; r++) {
                const cols: string[] = [];
                for (let c = 0; c < numCols; c++) {
                    const cell = getCell(cells[r * numCols + c]);
                    cols.push(cell?.computedValue ?? "");
                }
                rows.push(cols.join("\t"));
            }
            text = rows.join("\n");
        } else {
            const cell = getCell(cells[0]);
            text = cell?.computedValue ?? "";
        }

        navigator.clipboard.writeText(text);
    }

    async function pasteFromClipboard() {
        if (!focusedCell) return;
        const text = await navigator.clipboard.readText();
        if (!text) return;

        const bounds = primarySelection;
        const hasRange =
            !!bounds &&
            (bounds.maxR !== bounds.minR || bounds.maxC !== bounds.minC);

        // parse clipboard as TSV grid
        const clipRows = text.split("\n").map((line) => line.split("\t"));
        const isSingleClipValue =
            clipRows.length === 1 && clipRows[0].length === 1;
        const pairs: [CellId, string][] = [];

        // single value into single focused cell
        if (isSingleClipValue && !hasRange) {
            pairs.push([focusedCell, clipRows[0][0]]);
        }

        // single value into focused range: fill all cells
        if (isSingleClipValue && bounds && hasRange) {
            const val = clipRows[0][0];
            for (let r = bounds.minR; r <= bounds.maxR; r++) {
                for (let c = bounds.minC; c <= bounds.maxC; c++) {
                    pairs.push([{ row: r, col: c }, val]);
                }
            }
        }

        // range into single focused cell or focused range:
        // start from focusedCell (or top-left of range), expand right and down
        if (!isSingleClipValue) {
            const startRow = bounds && hasRange ? bounds.minR : focusedCell.row;
            const startCol = bounds && hasRange ? bounds.minC : focusedCell.col;
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
            focusedCell = { row: startRow, col: startCol };
            hoveredCell = { row: startRow, col: startCol };
            selections = [
                {
                    minR: startRow,
                    maxR: Math.min(
                        startRow + clipRows.length - 1,
                        rowCount - 1,
                    ),
                    minC: startCol,
                    maxC: Math.min(startCol + clipWidth - 1, columnCount - 1),
                },
            ];
        }

        if (!pairs.length) return;
        invoke("paste_values", { cells: pairs });
    }

    // --- grid config ---

    // todo: refactor all hardcoded values (like rowHeight, headerHeight, etc) into constants

    let left = 1; // pin first column (row numbers) to the left
    let select = false; // disable Grid's built-in selection, we handle it ourselves
    let filterValues = {};
    let sizes = {
        headerHeight: 28,
        rowHeight: 28,
    };

    function isInBounds(row: number, col: number): boolean {
        return row >= 0 && col >= 0 && row < rowCount && col < columnCount;
    }

    function clearFocus() {
        commitEdit();
        focusedCell = null;
        hoveredCell = null;
        selections = [];
        fillOriginalBounds = null;
        editorInput = "";
        isEditing = false;
        isSelecting = false;
        appendSelectionOnMouseUp = false;
        isSingleCellSelectionOnMouseUp = false;
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
                    `.wx-cell[data-row-id="${ui.row}"][data-col-id=":${ui.column}"] .editor`,
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
            if (!isSelecting && !editorInsertReference) {
                hoveredCell = null;
            }
            return;
        }

        const { rowId, colId } = clickedCell.dataset;

        // ignore row number column for hover tracking
        if (parseSvarID(colId) === "rowNumber") {
            if (!isSelecting && !editorInsertReference) {
                hoveredCell = null;
            }
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
        const bounds = primarySelection;
        if (!bounds) return;
        isFilling = true;
        isSelecting = true;
        fillOriginalBounds = { ...bounds };
        hoveredCell = { row: bounds.maxR, col: bounds.maxC };
    }

    function handleMouseDown(ev: MouseEvent) {
        // right-click inside current selection: let context menu handle it
        if (
            ev.button === 2 &&
            hoveredCell &&
            (isCellInAnyRange(hoveredCell, selections) ||
                (!!focusedCell &&
                    hoveredCell.row === focusedCell.row &&
                    hoveredCell.col === focusedCell.col))
        ) {
            return;
        }

        const target = ev.target as HTMLElement;
        const clickedCell = target.closest<HTMLElement>(".wx-cell");

        if (!clickedCell) return;
        const { rowId, colId } = clickedCell.dataset;
        if (!rowId || !colId || parseSvarID(colId) === "rowNumber") return;

        const clicked = domToCellId(rowId, colId);
        hoveredCell = clicked;

        // if clicked on already focused cell (and not on any interactive element inside the cell),
        // then start editing it
        if (
            !ev.ctrlKey &&
            !ev.shiftKey &&
            !ev.altKey &&
            focusedCell &&
            focusedCell.row === clicked.row &&
            focusedCell.col === clicked.col
        ) {
            if (
                target === clickedCell ||
                target.classList.contains("display-cell")
            ) {
                isEditing = true;
                moveFocusToInlineEditor(focusedCell);
            }
            return;
        }

        // if clicked while holding alt, move focus only
        if (ev.altKey) {
            if (isEditing) {
                commitEdit();
            }
            focusedCell = clicked;
            hoveredCell = clicked;
            isSelecting = false;
            appendSelectionOnMouseUp = false;
            isSingleCellSelectionOnMouseUp = false;
            if (!ev.ctrlKey) {
                selections = [];
            }
            return;
        }

        // while editing formula, insert reference from clicked cell and extend via drag
        if (isEditing && editorInputIsFormula) {
            ev.preventDefault();
            editorInsertReference = true;
            editorInsertReferenceStart = { ...clicked };
            editorInsertReferenceEnd = { ...clicked };
            return;
        }

        if (isEditing) {
            commitEdit();
        }

        isEditing = false;
        isSelecting = true;
        appendSelectionOnMouseUp = ev.ctrlKey;
        isSingleCellSelectionOnMouseUp = ev.ctrlKey;

        if (ev.shiftKey && focusedCell) {
            appendSelectionOnMouseUp = false;
            isSingleCellSelectionOnMouseUp = false;
            return;
        }

        if (ev.ctrlKey && focusedCell) {
            const prevFocusedCell = { ...focusedCell };
            if (!isCellInAnyRange(prevFocusedCell, selections)) {
                selections = [...selections, cellToRange(prevFocusedCell)];
            }
        }

        focusedCell = clicked;
        hoveredCell = clicked;
    }

    function handleMouseUp(ev: MouseEvent) {
        // fill cells on release
        if (isFilling && fillOriginalBounds && hoveredCell) {
            const bounds = rangeFromBoundsAndCell(
                fillOriginalBounds,
                hoveredCell,
            );
            const orig = fillOriginalBounds;

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

            const deleteCells = cellsOutside(orig, bounds);
            if (deleteCells.length) commitDelete(deleteCells);

            selections = [bounds];
            isFilling = false;
            fillOriginalBounds = null;
        } else if (isSelecting && focusedCell) {
            const bounds = rangeFromCells(
                focusedCell,
                hoveredCell ?? focusedCell,
            );
            const isSingleCell =
                bounds.minR === bounds.maxR && bounds.minC === bounds.maxC;

            if (appendSelectionOnMouseUp) {
                if (!isSingleCell || isSingleCellSelectionOnMouseUp) {
                    selections = [...selections, bounds];
                }
            } else if (!isSingleCell) {
                selections = [bounds];
            } else {
                selections = [];
            }
        }

        // stop selecting and inserting on mouse button release
        isSelecting = false;
        appendSelectionOnMouseUp = false;
        isSingleCellSelectionOnMouseUp = false;
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
        if ((colId && parseSvarID(colId) === "rowNumber") || isHeader) {
            clearFocus();
            return;
        }
    }

    function handleKeyDown(ev: KeyboardEvent) {
        // let overlay elements (e.g. editable table title) handle their own keys
        if (overlaysEl?.contains(ev.target as Node)) return;

        if (ev.ctrlKey && ev.shiftKey && ev.key === "Z") {
            commitRedo();
            return;
        }

        if (ev.ctrlKey && !ev.shiftKey && ev.key === "z") {
            commitUndo();
            return;
        }

        if (ev.ctrlKey && ev.shiftKey && ev.key === "C" && !isEditing) {
            ev.preventDefault();
            copySelectionValues();
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
                const destBounds = primarySelection;

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
            if (primarySelection) {
                clonedFormulaBounds = { ...primarySelection };
            }

            clearFocus();

            return;
        }

        if (ev.key === "Escape") {
            isEditing = false;
            clonedFormulaBounds = null;
            clearFocus();
            return;
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

            const current =
                ev.shiftKey && hoveredCell ? hoveredCell : focusedCell;
            const nextRow = current.row + rowDelta;
            const nextCol = current.col + colDelta;

            // if pressing alt + arrow key in edit mode ...
            if (isEditing && ev.altKey) {
                // ... then exit edit mode and move to cell in arrow direction
                commitEdit();
                isEditing = false;
                if (!isInBounds(nextRow, nextCol)) return;
            }

            if (!isInBounds(nextRow, nextCol)) {
                return;
            }

            if (ev.shiftKey) {
                const nextHoveredCell = { row: nextRow, col: nextCol };
                hoveredCell = nextHoveredCell;
                isSelecting = true;
                selections = [rangeFromCells(focusedCell, nextHoveredCell)];
                return;
            }

            isSelecting = false;
            focusedCell = { row: nextRow, col: nextCol };
            hoveredCell = { row: nextRow, col: nextCol };
            selections = [];
            return;
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
                    hoveredCell = { row: nextRow, col: focusedCell.col };
                    selections = [];
                }
            }
            // on delete, clear value in focus or in selected range
            else if (ev.key === "Delete") {
                ev.preventDefault();

                const selectedCells = getSelectedCells();
                if (selectedCells.length) {
                    commitDelete(selectedCells);
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
        // let overlay elements (e.g. editable table title) handle their own keys
        if (overlaysEl?.contains(ev.target as Node)) return;

        if (ev.key === "Shift" && !isEditing) {
            isSelecting = false;
        }
    }

    // --- scroll handling & selection overlays ---

    let sos: SheetObjectsState = $state(null as any);
    let focusOverlay = $state<FocusOverlay>(null as any);
    let selectionOverlays: (SelectionOverlay | undefined)[] = $state([]);
    let fillOriginOverlay = $state<FillOriginOverlay>(null as any);
    let cloneSourceOverlay = $state<CloneSourceOverlay>(null as any);
    let refOverlays: RefOverlay[] = $state([]);
    let tableOverlays: (TableOverlay | undefined)[] = $state([]);
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

    /** Scroll the grid so that the given 0-indexed column is visible. */
    function scrollToColumn(col: number) {
        const el = getScrollContainer();
        if (!el) return;
        const left = col * COL_WIDTH;
        if (left < el.scrollLeft) {
            el.scrollLeft = left;
        } else if (left + COL_WIDTH > el.scrollLeft + el.clientWidth) {
            el.scrollLeft = left + COL_WIDTH - el.clientWidth;
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
        const THRESHOLD = 50;
        showAddRows =
            scroller.scrollTop + scroller.clientHeight >=
            scroller.scrollHeight - THRESHOLD;
        showAddCols =
            scroller.scrollLeft + scroller.clientWidth >=
            scroller.scrollWidth - THRESHOLD;
        if (overlayDebounceId) clearTimeout(overlayDebounceId);
        overlayDebounceId = setTimeout(() => {
            applyHeaderHighlights();
            overlayDebounceId = null;
        }, 10);
        moveFocusBackToSpreadsheet();
    }

    function applyHeaderHighlights() {
        const wrapper = document.querySelector(".grid-wrapper");
        if (!wrapper) return;
        const highlightBounds = [
            ...selections,
            ...(focusedCellBounds ? [focusedCellBounds] : []),
            ...(isSelecting && activeSelectionBounds
                ? [activeSelectionBounds]
                : []),
        ];

        for (const col of wrapper.querySelectorAll<HTMLElement>(
            "[data-header-id]",
        )) {
            const colIdx = columnLetterToIndex(
                parseSvarID(col.dataset.headerId!) as string,
            );
            col.classList.toggle(
                "highlight-col",
                highlightBounds.some(
                    (bounds) => colIdx >= bounds.minC && colIdx <= bounds.maxC,
                ),
            );
        }

        for (const cell of wrapper.querySelectorAll<HTMLElement>(
            '.wx-cell[data-col-id=":rowNumber"]',
        )) {
            const rowIdx = (parseSvarID(cell.dataset.rowId!) as number) - 1;
            cell.classList.toggle(
                "highlight-row",
                highlightBounds.some(
                    (bounds) => rowIdx >= bounds.minR && rowIdx <= bounds.maxR,
                ),
            );
        }
    }

    function repositionOverlays() {
        if (!gridApi || !sos) return;
        const scroller = getScrollContainer();
        reposition(sos, scroller?.scrollLeft ?? 0, scroller?.scrollTop ?? 0);
        focusOverlay?.reposition();
        for (const selection of selectionOverlays) selection?.reposition();
        fillOriginOverlay?.reposition();
        cloneSourceOverlay?.reposition();
        for (const ref of refOverlays) ref?.reposition();
        for (const t of tableOverlays) t?.reposition();
    }

    // when selection or formula bounds change, reposition overlays and apply header highlight
    $effect(() => {
        focusedCellBounds;
        activeSelectionBounds;
        activeFocusHasBorder;
        activeFocusHasBackground;
        selections;
        fillOriginalBounds;
        clonedFormulaBounds;
        parsedFormulaReferencesHighlights;
        applyHeaderHighlights();
        repositionOverlays();
    });

    export function saveDecorationsToJson(): string {
        const gridState = gridApi!.getState();
        const columns: any[] = gridState._columns ?? [];

        const columnWidths: Record<string, number> = {};
        for (const col of columns) {
            columnWidths[col.id] = col.width;
        }

        // todo: row heights
        const rowHeights: Record<string, number> = {};

        return JSON.stringify({
            tables,
            defaultColWidth: COL_WIDTH,
            defaultRowHeight: sizes.rowHeight,
            columnWidths,
            rowHeights,
            rowCount,
            columnCount,
        });
    }

    export function onFileLoad(decorationsJson?: string) {
        // reset grid data
        baseRows.length = 0;
        for (let i = 0; i < INITIAL_ROWS; i++) {
            baseRows.push(makeRow(i, columnCount));
        }

        // reset selection and editing state
        focusedCell = null;
        hoveredCell = null;
        selections = [];
        isSelecting = false;
        appendSelectionOnMouseUp = false;
        isSingleCellSelectionOnMouseUp = false;
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

        // restore decorations
        if (decorationsJson) {
            const dec = JSON.parse(decorationsJson);
            if (dec.tables) tables = dec.tables;
            if (dec.rowCount && dec.rowCount > rowCount)
                expandRows(dec.rowCount);
            if (dec.columnCount && dec.columnCount > columnCount)
                expandColumns(dec.columnCount);
        } else {
            tables = [];
        }

        // reset all column widths to default, then apply saved widths
        const savedWidths = decorationsJson
            ? JSON.parse(decorationsJson).columnWidths
            : undefined;
        if (gridApi) {
            for (const col of gridApi.getState()._columns ?? []) {
                const defaultW =
                    col.id === "rowNumber" ? ROW_NUMBER_WIDTH : COL_WIDTH;
                const targetW = savedWidths?.[col.id!] ?? defaultW;
                if (col.width !== targetW) {
                    gridApi.exec("resize-column", {
                        id: col.id!,
                        width: targetW,
                    });
                }
            }
        }

        updateVisibleColumns();
        restartPolling();
    }

    let pollInterval: ReturnType<typeof setInterval> | undefined;

    function setViewportRows(start: number, end: number) {
        gridRows = baseRows.slice(start, end + 1);
        viewportRowStart = start;
        viewportRowEnd = end;
    }

    function pollViewportOnce() {
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
                    editorInput = decodeEditorValue(new Uint8Array(response));
                }
            });
        }

        if (focusedCell && !isEditingCellName) {
            invoke<ArrayBuffer>("get_name_for_cell", {
                cellId: focusedCell,
            }).then((response) => {
                if (!isEditingCellName) {
                    cellName = new TextDecoder().decode(
                        new Uint8Array(response),
                    );
                }
            });
        }
    }

    function restartPolling() {
        if (pollInterval !== undefined) clearInterval(pollInterval);

        invoke("init_viewport").then(() => {
            requestAnimationFrame(() => {
                updateVisibleColumns();
                pollViewportOnce();

                pollInterval = setInterval(pollViewportOnce, 16);
            });
        });
    }

    onMount(() => {
        initOverlays();
        updateVisibleColumns();
        restartPolling();

        const unlistenEnabled = listen<number>(
            "enable-table-projection",
            (event) => {
                const tableId = event.payload;
                tables = tables.map((t) =>
                    t.id === tableId ? { ...t, hasProjection: true } : t,
                );
            },
        );
        const unlistenDisabled = listen<number>(
            "disable-table-projection",
            (event) => {
                const tableId = event.payload;
                tables = tables.map((t) =>
                    t.id === tableId
                        ? { ...t, hasProjection: false, hiddenRowsCount: 0 }
                        : t,
                );
            },
        );
        const unlistenHiddenRows = listen<[number, number]>(
            "update-table-hidden-rows",
            (event) => {
                const [tableId, hiddenRowsCount] = event.payload;
                tables = tables.map((t) =>
                    t.id === tableId ? { ...t, hiddenRowsCount } : t,
                );
            },
        );

        return () => {
            if (pollInterval !== undefined) clearInterval(pollInterval);
            unlistenEnabled.then((fn) => fn());
            unlistenDisabled.then((fn) => fn());
            unlistenHiddenRows.then((fn) => fn());
        };
    });

    function handleRequestData(
        ev: { row: { start: number; end: number } } & { [key: string]: any },
    ): void {
        const {
            row: { start, end },
        } = ev;
        setViewportRows(start, end);
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
            {filterValues}
            {cellStyle}
        />
    </ContextMenu>
    {#if showAddRows}
        <div class="add-rows-bar">
            <button
                onclick={() => {
                    const count = parseInt(String(addRowsCount), 10);
                    if (!count || count < 1) return;
                    const newRowCount = rowCount + count;
                    expandRows(newRowCount);
                    requestAnimationFrame(() => {
                        scrollToRow(newRowCount);
                    });
                }}>Add</button
            >
            <input
                inputmode="numeric"
                pattern="[0-9]*"
                bind:value={addRowsCount}
                min="1"
            />
            <span>rows</span>
        </div>
    {/if}
    {#if showAddCols}
        <div class="add-cols-bar">
            <button
                onclick={() => {
                    const count = parseInt(String(addColsCount), 10);
                    if (!count || count < 1) return;
                    const newColumnCount = columnCount + count;
                    expandColumns(newColumnCount);
                    requestAnimationFrame(() => {
                        scrollToColumn(newColumnCount);
                    });
                }}>Add</button
            >
            <input
                inputmode="numeric"
                pattern="[0-9]*"
                bind:value={addColsCount}
                min="1"
            />
            <span>columns</span>
        </div>
    {/if}
    <div class="selection-overlays-clip" bind:this={clipWrapperEl}>
        <div class="selection-overlays" bind:this={overlaysEl}>
            {#if sos}
                {#each tables as table, i}
                    <TableOverlay bind:this={tableOverlays[i]} {sos} {table} />
                {/each}
                {#each selections as bounds, i}
                    <SelectionOverlay
                        bind:this={selectionOverlays[i]}
                        {sos}
                        {bounds}
                        visible={true}
                    />
                {/each}
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
                    bounds={activeSelectionBounds}
                    visible={!!activeSelectionBounds}
                    {isFilling}
                    {isEditing}
                    {editorInputWidth}
                    showBorder={activeFocusHasBorder}
                    showBackground={activeFocusHasBackground}
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

    .add-rows-bar,
    .add-cols-bar {
        position: absolute;
        z-index: 6;
        display: flex;
        align-items: center;
        gap: 0.5em;
        padding: 0.75em 1.5em;
        font-size: 0.8rem;
        background: var(--wx-table-header-background);
        border-radius: 0.4em;
    }

    .add-rows-bar button,
    .add-cols-bar button,
    .add-rows-bar input,
    .add-cols-bar input {
        all: unset;
        font: inherit;
        color: inherit;
        padding: 0.35em 0.6em;
        border-radius: 0.25em;
        background: rgba(255, 255, 255, 0.07);
    }

    .add-rows-bar button,
    .add-cols-bar button {
        cursor: pointer;
    }

    .add-rows-bar button:hover,
    .add-cols-bar button:hover {
        background: rgba(255, 255, 255, 0.13);
    }

    .add-rows-bar input,
    .add-cols-bar input {
        width: 4em;
        text-align: center;
    }

    .add-rows-bar {
        bottom: 0.8em;
        left: 50%;
        transform: translateX(-50%);
        animation: fade-in-up 150ms ease-out;
    }

    .add-cols-bar {
        right: 0.8em;
        top: 50%;
        transform: translateY(-50%);
        animation: fade-in-left 150ms ease-out;
    }

    @keyframes fade-in-up {
        from {
            opacity: 0;
            transform: translateX(-50%) translateY(6px);
        }
        to {
            opacity: 1;
            transform: translateX(-50%) translateY(0);
        }
    }

    @keyframes fade-in-left {
        from {
            opacity: 0;
            transform: translateY(-50%) translateX(6px);
        }
        to {
            opacity: 1;
            transform: translateY(-50%) translateX(0);
        }
    }

    /* Static clip wrapper - prevents overlays from rendering over headers/row numbers */
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

    :global(.wx-cell[data-col-id=":rowNumber"]) {
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

    :global(div[role="columnheader"][data-header-id=":rowNumber"]) {
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

    :global(.table-header-cell) {
        background: #1a1a1a !important;
        border-bottom: 1px solid #333 !important;
        border-right: 0px !important;
        font-weight: 500;
        color: #a1a1aa;
    }

    :global(.table-row-even) {
        background: #1e1f22 !important;
        border-bottom: 1px solid #2a2a2a !important;
    }

    :global(.table-row-odd) {
        background: #232527 !important;
        border-bottom: 1px solid #2a2a2a !important;
    }
</style>
