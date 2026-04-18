<script lang="ts">
    import type { Snippet } from "svelte";
    import { type IApi } from "@svar-ui/svelte-grid";
    import {
        columnIndexToLetter,
        columnLetterToIndex,
        parseSvarID,
        setSheetSharedState,
        type CellData,
        type CellId,
        type ChangeBounds,
        type TableData,
        type UICell,
    } from "$lib/sheet/shared";
    import { invoke } from "@tauri-apps/api/core";
    import SheetFormulaPanel from "./SheetFormulaPanel.svelte";
    import { onMount, tick, untrack } from "svelte";
    import {
        cellsInBounds,
        cellsOutside,
        type CellRange,
    } from "./overlays/Overlays.svelte";
    import { listen } from "@tauri-apps/api/event";
    import { ContextMenu, type IMenuOptionClick } from "@svar-ui/svelte-menu";
    import { initNotice } from "$lib/notice";
    import { showError } from "$lib/notice";
    import { getContext } from "svelte";
    import Render from "./Render.svelte";

    let { sidebar }: { sidebar?: Snippet } = $props();

    const helpers = getContext<{ showNotice: (msg: any) => void }>(
        "wx-helpers",
    );
    if (helpers) initNotice(helpers.showNotice);

    type ContextMenuTarget =
        | { kind: "cell"; cell: CellId }
        | { kind: "row"; row: number }
        | { kind: "column"; col: number };

    let contextMenuTarget: ContextMenuTarget | null = $state(null);

    let contextMenuOptions = $derived.by(() => {
        if (contextMenuTarget?.kind === "column") {
            return [
                { id: "insert-column-left", text: "Insert 1 column left" },
                { id: "insert-column-right", text: "Insert 1 column right" },
                { id: "remove-column", text: "Remove column" },
            ];
        }

        if (contextMenuTarget?.kind === "row") {
            return [
                { id: "insert-row-above", text: "Insert 1 row above" },
                { id: "insert-row-below", text: "Insert 1 row below" },
                { id: "remove-row", text: "Remove row" },
            ];
        }

        return [
            { id: "copy", text: "Copy", icon: "wxi wxi-content-copy" },
            {
                id: "copy-values",
                text: "Copy Values",
                icon: "wxi wxi-content-copy",
            },
            { id: "paste", text: "Paste", icon: "wxi wxi-content-paste" },
            { id: "create-table", text: "Create Table" },
        ];
    });

    async function handleContextMenuClick(ev: IMenuOptionClick) {
        if (!ev.option) return;

        // todo: reduce code duplication below

        if (
            ev.option.id === "insert-column-left" &&
            contextMenuTarget?.kind === "column"
        ) {
            const col = contextMenuTarget.col;
            invoke("insert_column", { col, left: true })
                .then(() => {
                    columnCount += 1;
                    if (rowCount > 0) {
                        selectRange(
                            { row: 0, col },
                            {
                                minR: 0,
                                maxR: rowCount - 1,
                                minC: col,
                                maxC: col,
                            },
                        );
                    }
                    requestAnimationFrame(() => render?.repositionOverlays());
                })
                .catch((e) => showError(String(e)));
            return;
        }

        if (
            ev.option.id === "insert-column-right" &&
            contextMenuTarget?.kind === "column"
        ) {
            const col = contextMenuTarget.col + 1;
            invoke("insert_column", { col: contextMenuTarget.col, left: false })
                .then(() => {
                    columnCount += 1;
                    if (rowCount > 0) {
                        selectRange(
                            { row: 0, col },
                            {
                                minR: 0,
                                maxR: rowCount - 1,
                                minC: col,
                                maxC: col,
                            },
                        );
                    }
                    requestAnimationFrame(() => render?.repositionOverlays());
                })
                .catch((e) => showError(String(e)));
            return;
        }

        if (
            ev.option.id === "insert-row-above" &&
            contextMenuTarget?.kind === "row"
        ) {
            const row = contextMenuTarget.row;
            invoke("insert_row", { row: contextMenuTarget.row, below: false })
                .then(() => {
                    rowCount += 1;
                    if (columnCount > 0) {
                        selectRange(
                            { row, col: 0 },
                            {
                                minR: row,
                                maxR: row,
                                minC: 0,
                                maxC: columnCount - 1,
                            },
                        );
                    }
                    requestAnimationFrame(() => render?.repositionOverlays());
                })
                .catch((e) => showError(String(e)));
            return;
        }

        if (
            ev.option.id === "insert-row-below" &&
            contextMenuTarget?.kind === "row"
        ) {
            const row = contextMenuTarget.row + 1;
            invoke("insert_row", { row: contextMenuTarget.row, below: true })
                .then(() => {
                    rowCount += 1;
                    if (columnCount > 0) {
                        selectRange(
                            { row, col: 0 },
                            {
                                minR: row,
                                maxR: row,
                                minC: 0,
                                maxC: columnCount - 1,
                            },
                        );
                    }
                    requestAnimationFrame(() => render?.repositionOverlays());
                })
                .catch((e) => showError(String(e)));
            return;
        }

        if (
            ev.option.id === "remove-column" &&
            contextMenuTarget?.kind === "column"
        ) {
            const removedCol = contextMenuTarget.col;
            invoke("remove_column", { col: removedCol })
                .then(() => {
                    columnCount = Math.max(0, columnCount - 1);
                    if (rowCount > 0 && columnCount > 0) {
                        const col = Math.min(removedCol, columnCount - 1);
                        selectRange(
                            { row: 0, col },
                            {
                                minR: 0,
                                maxR: rowCount - 1,
                                minC: col,
                                maxC: col,
                            },
                        );
                    } else {
                        clearFocus();
                    }
                    requestAnimationFrame(() => render?.repositionOverlays());
                })
                .catch((e) => showError(String(e)));
            return;
        }

        if (
            ev.option.id === "remove-row" &&
            contextMenuTarget?.kind === "row"
        ) {
            const removedRow = contextMenuTarget.row;
            invoke("remove_row", { row: removedRow })
                .then(() => {
                    rowCount = Math.max(0, rowCount - 1);
                    if (columnCount > 0 && rowCount > 0) {
                        const row = Math.min(removedRow, rowCount - 1);
                        selectRange(
                            { row, col: 0 },
                            {
                                minR: row,
                                maxR: row,
                                minC: 0,
                                maxC: columnCount - 1,
                            },
                        );
                    } else {
                        clearFocus();
                    }
                    requestAnimationFrame(() => render?.repositionOverlays());
                })
                .catch((e) => showError(String(e)));
            return;
        }

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
                requestAnimationFrame(() => render?.repositionOverlays());
            })
            .catch((e) => showError(String(e)));
    }

    function contextMenuResolver(_: any, event: MouseEvent) {
        // before the grid's context menu opens, ...
        const target = event.target as HTMLElement;
        contextMenuTarget = null;

        // if right-clicked column header, select full column and show insert actions
        const clickedHeader = target.closest<HTMLElement>(
            "[role='columnheader']",
        );
        if (clickedHeader) {
            const headerId = parseSvarID(clickedHeader.dataset.headerId);
            if (typeof headerId !== "string" || headerId === "rowNumber") {
                return null;
            }

            const col = columnLetterToIndex(headerId);
            if (rowCount > 0) {
                selectRange(
                    { row: 0, col },
                    { minR: 0, maxR: rowCount - 1, minC: col, maxC: col },
                );
            }
            contextMenuTarget = { kind: "column", col };
            return contextMenuTarget;
        }

        const el = target.closest<HTMLElement>(".wx-cell");
        if (!el) return null;

        const { rowId, colId } = el.dataset;
        if (!rowId || !colId) return null;

        // if right-clicked row header, select full row and show insert actions
        if (parseSvarID(colId) === "rowNumber") {
            const parsedRow = parseSvarID(rowId);
            if (typeof parsedRow !== "number") return null;

            const row = parsedRow - 1;
            if (columnCount > 0) {
                selectRange(
                    { row, col: 0 },
                    {
                        minR: row,
                        maxR: row,
                        minC: 0,
                        maxC: columnCount - 1,
                    },
                );
            }
            contextMenuTarget = { kind: "row", row };
            return contextMenuTarget;
        }

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

        contextMenuTarget = { kind: "cell", cell: clicked };
        return contextMenuTarget;
    }

    // -- backend (tauri) communication setup --

    const textDecoder = new TextDecoder();

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
    let cellStyles = $derived.by(() => {
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

    const INITIAL_ROWS = 1000;
    const INITIAL_COLS = 26;

    let rowCount = $state(INITIAL_ROWS);
    let columnCount = $state(INITIAL_COLS);

    // --- Render component ref ---

    let render: Render | null = $state(null);
    let gridApi: IApi | null = $state(null);
    function getOverlaysEl(): HTMLElement | null {
        return render?.getOverlaysEl() ?? null;
    }

    function isOverlayEvent(ev: Event): boolean {
        return !!getOverlaysEl()?.contains(ev.target as Node);
    }

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

    // --- cell expand ---
    let expandedCells: Map<
        string,
        import("$lib/sheet/shared").CellFormattingData
    > = $state(new Map());
    let expandModeActive = $state(false);

    function detectDoubleKey(): boolean {
        const now = Date.now();
        if (now - _lastKeyUpTime < 300) {
            _lastKeyUpTime = 0;
            return true;
        }
        _lastKeyUpTime = now;
        return false;
    }
    let _lastKeyUpTime = 0;

    const REF_COLORS = [
        "#4184BF",
        "#F27405",
        "#08A64D",
        "#F2A516",
        "#4B93BF",
        "#d3869b",
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
        get hoveredCell() {
            return hoveredCell;
        },
        set hoveredCell(v) {
            hoveredCell = v;
        },
        get selections() {
            return selections;
        },
        set selections(v) {
            selections = v;
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
        get cellStyles() {
            return cellStyles;
        },
        get expandedCells() {
            return expandedCells;
        },
        set expandedCells(v) {
            expandedCells = v;
        },
        get expandModeActive() {
            return expandModeActive;
        },
        set expandModeActive(v) {
            expandModeActive = v;
        },
        getCellDisplayValue(row: number, col: number): string {
            const cell = render?.getCell({ row, col });
            return cell?.computedValue ?? "";
        },
        getCellIsFormula(row: number, col: number): boolean {
            const cell = render?.getCell({ row, col });
            return cell?.isFormula ?? false;
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

    function selectRange(anchor: CellId, bounds: CellRange) {
        if (!isInBounds(anchor.row, anchor.col)) return;
        if (isEditing) commitEdit();
        isEditing = false;
        focusedCell = { ...anchor };
        hoveredCell = { ...anchor };
        selections = [bounds];
        isSelecting = false;
        appendSelectionOnMouseUp = false;
        isSingleCellSelectionOnMouseUp = false;
        isFilling = false;
        fillOriginalBounds = null;
        editorInsertReference = false;
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

    let showFocusBorder = $derived.by(() => {
        if (!activeSelectionBounds) return false;
        if (isFilling) return true;
        if (isSelecting) return isSingleCellRange(activeSelectionBounds);
        return true;
    });

    let showFocusBackground = $derived.by(() => {
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

    function getCell(id: CellId | null): CellData | null {
        return render?.getCell(id) ?? null;
    }

    function commitEdit() {
        if (!focusedCell || !isEditing) return;
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

    function scrollToRow(row: number) {
        render?.scrollToRow(row);
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
            selectRange(
                { row: startRow, col: startCol },
                {
                    minR: startRow,
                    maxR: Math.min(
                        startRow + clipRows.length - 1,
                        rowCount - 1,
                    ),
                    minC: startCol,
                    maxC: Math.min(startCol + clipWidth - 1, columnCount - 1),
                },
            );
        }

        if (!pairs.length) return;
        invoke("paste_values", { cells: pairs });
    }

    // --- grid config ---

    // todo: refactor all hardcoded values (like rowHeight, headerHeight, etc) into constants

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
        expandModeActive = false;
        gridApi?.exec("focus-cell", {
            row: undefined,
            column: undefined,
        });
    }

    async function moveFocusToInlineEditor(cell: CellId) {
        // wait for Svelte to flush DOM updates (InputCell appears after isEditing becomes true)
        await tick();
        await new Promise((r) => requestAnimationFrame(r));

        const expandKey = `${cell.row},${cell.col}`;
        const hasExpand =
            expandedCells.has(expandKey) ||
            (expandModeActive &&
                focusedCell?.row === cell.row &&
                focusedCell?.col === cell.col);

        console.log(
            "[moveFocus] cell:",
            expandKey,
            "hasExpand:",
            hasExpand,
            "getOverlaysEl():",
            !!getOverlaysEl(),
            "expandedCells.has:",
            expandedCells.has(expandKey),
            "expandModeActive:",
            expandModeActive,
            "isEditing:",
            isEditing,
        );

        const oel = getOverlaysEl();
        if (hasExpand && oel) {
            const editor = oel.querySelector<HTMLInputElement>(
                ".expanded-cell .editor",
            );
            console.log(
                "[moveFocus] overlay editor found:",
                !!editor,
                "all .expanded-cell:",
                oel.querySelectorAll(".expanded-cell").length,
                "all .editor:",
                oel.querySelectorAll(".editor").length,
            );
            if (editor) {
                editor.focus();
                return;
            }
        }

        // regular grid cell
        const ui = toUICell(cell);
        const gridEditor = document.querySelector<HTMLInputElement>(
            `.wx-cell[data-row-id="${ui.row}"][data-col-id=":${ui.column}"] .editor`,
        );
        console.log("[moveFocus] grid editor found:", !!gridEditor);
        gridEditor?.focus();
    }

    function handleGridInit(api: IApi) {
        gridApi = api;
    }

    function handleMouseMove(ev: MouseEvent) {
        if (isOverlayEvent(ev)) return;

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
        // clicking anywhere on the grid (not on the overlay, which stopPropagates) exits resize mode
        if (expandModeActive) {
            expandModeActive = false;
        }

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

        // if clicked while holding alt, move focus only (discard any in-progress edit)
        if (ev.altKey) {
            isEditing = false;
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
        if (isOverlayEvent(ev)) return;

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
        const clickedHeader = target.closest<HTMLElement>(
            "[role='columnheader']",
        );
        if (clickedHeader) {
            const lastRow = rowCount - 1;
            const lastCol = columnCount - 1;
            const headerId = parseSvarID(clickedHeader.dataset.headerId);

            // if clicked top-left header, select whole sheet
            if (headerId === "rowNumber" && lastRow >= 0 && lastCol >= 0) {
                selectRange(
                    { row: 0, col: 0 },
                    { minR: 0, maxR: lastRow, minC: 0, maxC: lastCol },
                );
                return;
            }

            // otherwise, select whole column
            if (typeof headerId === "string") {
                const col = columnLetterToIndex(headerId);
                if (lastRow >= 0) {
                    selectRange(
                        { row: 0, col },
                        { minR: 0, maxR: lastRow, minC: col, maxC: col },
                    );
                }
                return;
            }
        }

        const clickedCell = target.closest<HTMLElement>(".wx-cell");
        if (!clickedCell) {
            if (hoveredCell) hoveredCell = null;
            return;
        }

        const { rowId, colId } = clickedCell.dataset;

        // if clicked row header, select whole row
        if (colId && parseSvarID(colId) === "rowNumber") {
            const row = Number(rowId) - 1;
            const lastCol = columnCount - 1;
            if (lastCol >= 0) {
                selectRange(
                    { row, col: 0 },
                    { minR: row, maxR: row, minC: 0, maxC: lastCol },
                );
            }
            return;
        }
    }

    function handleKeyDown(ev: KeyboardEvent) {
        if (isOverlayEvent(ev)) return;

        if (ev.ctrlKey && ev.shiftKey && ev.key === "Z") {
            commitRedo();
            return;
        }

        if (ev.ctrlKey && !ev.shiftKey && ev.key === "z") {
            commitUndo();
            return;
        }

        // Ctrl+A: select whole sheet
        if (
            ev.ctrlKey &&
            !ev.altKey &&
            !isEditing &&
            ev.key.toLowerCase() === "a"
        ) {
            ev.preventDefault();
            if (rowCount > 0 && columnCount > 0) {
                selectRange(
                    { row: 0, col: 0 },
                    {
                        minR: 0,
                        maxR: rowCount - 1,
                        minC: 0,
                        maxC: columnCount - 1,
                    },
                );
            }
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
            // arrow navigation exits expand mode
            if (expandModeActive) expandModeActive = false;
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
        if (isOverlayEvent(ev)) return;

        if (ev.key === "Shift" && !isEditing) {
            isSelecting = false;
            if (detectDoubleKey() && focusedCell) {
                expandModeActive = !expandModeActive;
            }
        }
    }

    // --- public API (delegated to Render or handled here) ---

    export function saveDecorationsToJson(): string {
        const base = JSON.parse(render?.saveDecorationsToJson() ?? "{}");
        if (expandedCells.size > 0) {
            const obj: Record<
                string,
                { extraWidth: number; extraHeight: number }
            > = {};
            for (const [k, v] of expandedCells) obj[k] = v;
            base.expandedCells = obj;
        }
        return JSON.stringify(base);
    }

    export function onFileLoad(decorationsJson?: string) {
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
        expandModeActive = false;

        // restore decorations
        if (decorationsJson) {
            const dec = JSON.parse(decorationsJson);
            if (dec.tables) tables = dec.tables;
            if (dec.expandedCells) {
                const map = new Map<
                    string,
                    { extraWidth: number; extraHeight: number }
                >();
                for (const [k, v] of Object.entries(dec.expandedCells))
                    map.set(k, v as any);
                expandedCells = map;
            } else {
                expandedCells = new Map();
            }
        } else {
            tables = [];
            expandedCells = new Map();
        }

        // delegate data/grid/polling reset to Render
        render?.onFileLoad(decorationsJson);
    }

    // --- Tauri event listeners ---

    onMount(() => {
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
            unlistenEnabled.then((fn) => fn());
            unlistenDisabled.then((fn) => fn());
            unlistenHiddenRows.then((fn) => fn());
        };
    });
</script>

<SheetFormulaPanel />

<div class="grid-row">
    <div
        class="grid-wrapper"
        onmousemove={handleMouseMove}
        onmouseup={handleMouseUp}
        onmousedown={handleMouseDown}
        onkeydowncapture={handleKeyDown}
        onkeyupcapture={handleKeyUp}
        tabindex="-1"
        role="grid"
    >
        <ContextMenu
            options={contextMenuOptions}
            onclick={handleContextMenuClick}
            at="point"
            resolver={contextMenuResolver}
        >
            <Render
                bind:this={render}
                {isFilling}
                {fillOriginalBounds}
                {clonedFormulaBounds}
                {parsedFormulaReferencesHighlights}
                {activeRefIndex}
                {selections}
                {activeSelectionBounds}
                activeFocusHasBorder={showFocusBorder}
                activeFocusHasBackground={showFocusBackground}
                {focusedCellBounds}
                {isSelecting}
                bind:rowCount
                bind:columnCount
                onfillstart={handleFillStart}
                oninit={handleGridInit}
            />
        </ContextMenu>
    </div>
    {#if sidebar}
        {@render sidebar()}
    {/if}
</div>

<style>
    .grid-row {
        flex: 1 1 auto;
        min-height: 0;
        display: flex;
    }

    .grid-wrapper {
        flex: 1 1 auto;
        min-height: 0;
        min-width: 0;
        position: relative;
        overflow: hidden;
        outline: none;
    }

    :global(.wx-header) {
        border-bottom: none !important;
    }

    :global(.wx-table-box) {
        border-top: none !important;
        background: var(--tonic-content-bg, var(--wx-background)) !important;
    }

    :global(.wx-cell[data-col-id=":rowNumber"]) {
        background: var(--wx-table-header-background) !important;
        font-weight: var(--wx-header-font-weight) !important;
        font-family: "JetBrains Mono", monospace;
        font-size: 10px;
        color: var(--tonic-row-num-color);
        text-align: center;
        display: flex !important;
        align-items: center;
        justify-content: center;
        user-select: none;
        border-right: var(--wx-table-cell-border) !important;
        -webkit-user-select: none;
    }

    :global(div[role="columnheader"][data-header-id=":rowNumber"]) {
        border-right: var(--wx-table-cell-border) !important;
    }

    :global(div[role="columnheader"]) {
        font-family: "JetBrains Mono", monospace;
        font-size: 10px;
        letter-spacing: 0.04em;
        color: var(--tonic-col-header-color);
        background: var(--wx-table-header-background) !important;
        border-bottom: var(
            --wx-table-header-border,
            var(--wx-border)
        ) !important;
        border-right: var(
            --wx-table-header-cell-border,
            var(--wx-border)
        ) !important;
    }

    :global(div[role="columnheader"].highlight-col) {
        box-shadow: inset 0 -2px 0 var(--wx-color-primary) !important;
        background-color: var(--tonic-highlight-bg) !important;
        color: var(--wx-color-primary) !important;
    }

    :global(.wx-cell.highlight-row) {
        box-shadow: inset 2px 0 0 var(--wx-color-primary) !important;
        background-color: var(--tonic-highlight-bg) !important;
        color: var(--wx-color-primary) !important;
    }

    :global(.wx-cell:focus) {
        outline: 0px !important;
    }

    :global(.table-header-cell) {
        background: var(--tonic-table-header-cell-bg) !important;
        border-bottom: 1px solid var(--tonic-table-header-cell-border-color) !important;
        border-right: 0px !important;
        font-weight: 500;
        color: var(--tonic-table-header-cell-color);
    }

    :global(.table-row-even) {
        background: #1e2026 !important;
        border-bottom: 1px solid #35373d !important;
    }

    :global(.table-row-odd) {
        background: #22242a !important;
        border-bottom: 1px solid #35373d !important;
    }

    :global([data-theme="light"] .table-row-even) {
        background: #f6f6f7 !important;
        border-bottom: 1px solid #d2d3d6 !important;
    }

    :global([data-theme="light"] .table-row-odd) {
        background: #eaebec !important;
        border-bottom: 1px solid #d2d3d6 !important;
    }
</style>
