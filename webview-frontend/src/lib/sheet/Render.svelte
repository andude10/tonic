<script lang="ts">
    import { Grid, type IApi, type IColumnConfig } from "@svar-ui/svelte-grid";
    import {
        columnIndexToLetter,
        columnLetterToIndex,
        getSheetSharedState,
        isCellData,
        parseSvarID,
        REF_COLORS,
        type CellData,
        type CellId,
        type SheetRow,
    } from "$lib/sheet/shared";
    import { invoke } from "@tauri-apps/api/core";
    import Cell from "./Cell.svelte";
    import { onMount } from "svelte";
    import {
        createState,
        syncScroll,
        reposition,
        type CellRange,
        type SheetObjectsState,
    } from "./overlays/Overlays.svelte";
    import SelectionOverlay from "./overlays/SelectionOverlay.svelte";
    import FocusOverlay from "./overlays/FocusOverlay.svelte";
    import FillOriginOverlay from "./overlays/FillOriginOverlay.svelte";
    import CloneSourceOverlay from "./overlays/CloneSourceOverlay.svelte";
    import RefOverlay from "./overlays/RefOverlay.svelte";
    import TableOverlay from "./overlays/TableOverlay.svelte";
    import ExpandedCellOverlay from "./overlays/ExpandedCellOverlay.svelte";

    type FormulaReferenceHighlight = {
        bounds: { minR: number; maxR: number; minC: number; maxC: number };
        colorIndex: number;
        isActive: boolean;
    };

    const shared = getSheetSharedState();

    // --- props ---

    let {
        isFilling,
        fillOriginalBounds,
        clonedFormulaBounds,
        parsedFormulaReferencesHighlights,
        activeRefIndex,
        selections,
        activeSelectionBounds,
        activeFocusHasBorder,
        activeFocusHasBackground,
        focusedCellBounds,
        isSelecting,
        rowCount = $bindable(),
        columnCount = $bindable(),
        onfillstart,
        oninit,
    }: {
        isFilling: boolean;
        fillOriginalBounds: CellRange | null;
        clonedFormulaBounds: CellRange | null;
        parsedFormulaReferencesHighlights:
            | (FormulaReferenceHighlight & {
                  matchIndex: number;
                  matchLength: number;
              })[]
            | null;
        activeRefIndex: number;
        selections: CellRange[];
        activeSelectionBounds: CellRange | null;
        activeFocusHasBorder: boolean;
        activeFocusHasBackground: boolean;
        focusedCellBounds: CellRange | null;
        isSelecting: boolean;
        rowCount: number;
        columnCount: number;
        onfillstart: (ev: MouseEvent) => void;
        oninit: (api: IApi) => void;
    } = $props();

    // --- constants ---

    const COL_WIDTH = 90;
    const ROW_NUMBER_WIDTH = 50;

    const textDecoder = new TextDecoder();
    const EMPTY_BODY = new Uint8Array();

    let left = 1;
    let select = false;
    let sizes = { headerHeight: 28, rowHeight: 28 };

    // --- data model ---
    // viewportRows and viewportColumns are the only data the Grid sees.
    // Both are replaced with fresh arrays each render tick. Old arrays are GC'd.

    let viewportRows: SheetRow[] = $state([]);
    let viewportColumns: IColumnConfig[] = $state(buildColumns(columnCount));

    function buildColumns(count: number): IColumnConfig[] {
        const cols: IColumnConfig[] = [
            { id: "rowNumber", width: ROW_NUMBER_WIDTH, resize: true },
        ];
        for (let i = 0; i < count; i++) {
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
    }

    // Build a fresh row array for the current viewport range.
    // Each row has { id, rowNumber, A: {..}, B: {..}, ... } with empty cells.
    function buildViewportRows(
        start: number,
        end: number,
        colCount: number,
    ): SheetRow[] {
        const count = end - start + 1;
        const rows: SheetRow[] = new Array(count);
        for (let i = 0; i < count; i++) {
            const id = start + i + 1; // 1-indexed for SVAR Grid
            const row: SheetRow = { id, rowNumber: id };
            for (let c = 0; c < colCount; c++) {
                row[columnIndexToLetter(c)] = {
                    computedValue: "",
                    isFormula: false,
                };
            }
            rows[i] = row;
        }
        return rows;
    }

    let gridApi: IApi | null = $state(null);

    function cellStyle(row: any, col: any): string {
        return shared.cellStyles.get(`${row.id},${col.id}`) ?? "";
    }

    // --- viewport tracking ---

    let viewportRowStart = 0;
    let viewportRowEnd = 40;
    let viewportColumnStart = 0;
    let viewportColumnEnd = 25;

    function updateApproximateColumnViewportBounds() {
        const el = getScrollContainer();
        if (!el || !el.clientWidth) return;
        viewportColumnStart = (el.scrollLeft / COL_WIDTH) | 0;
        viewportColumnEnd = Math.min(
            ((el.scrollLeft + el.clientWidth) / COL_WIDTH + 0.999) | 0,
            columnCount - 1,
        );
    }

    // --- decode ---

    // Decode binary cell data directly into a fresh rows array.
    // rows[0] corresponds to rowStart, rows[1] to rowStart+1, etc.
    function decodeCellsInto(
        rows: SheetRow[],
        rowStart: number,
        bytes: Uint8Array,
        view: DataView,
    ) {
        let offset = 0;
        const len = bytes.byteLength;
        while (offset < len) {
            const row = view.getUint32(offset, true);
            const col = view.getUint32(offset + 4, true);
            const flags = bytes[offset + 8];
            const isFormula = (flags & 1) !== 0;
            const isPending = (flags & 2) !== 0;
            const isError = (flags & 4) !== 0;
            offset += 9;

            const displayLen = view.getUint32(offset, true);
            offset += 4;
            const displayStart = offset;
            offset += displayLen;

            const targetRow = rows[row - rowStart];
            if (!targetRow) continue;
            const cell = targetRow[columnIndexToLetter(col)];
            if (!cell || typeof cell !== "object") continue;

            if (displayLen) {
                cell.computedValue = textDecoder.decode(
                    bytes.subarray(displayStart, displayStart + displayLen),
                );
            }
            if (isFormula) cell.isFormula = true;
            if (isPending) cell.isPending = true;
            if (isError) {
                cell.isError = true;
                const msgLen = view.getUint32(offset, true);
                offset += 4;
                if (msgLen) {
                    cell.errorMessage = textDecoder.decode(
                        bytes.subarray(offset, offset + msgLen),
                    );
                    offset += msgLen;
                }
            }
        }
    }

    // --- render loop ---

    let _scrolling = false;

    function renderLoop() {
        const rowStart = viewportRowStart;
        const rowEnd = viewportRowEnd;
        const colStart = viewportColumnStart;
        const colEnd = viewportColumnEnd;
        const colCount = columnCount;

        const cellsPromise = invoke<ArrayBuffer>(
            "get_cells_in_viewport",
            EMPTY_BODY,
            {
                headers: {
                    "row-start": String(rowStart),
                    "row-end": String(rowEnd),
                    "col-start": String(colStart),
                    "col-end": String(colEnd),
                },
            },
        );

        const editorPromise = shared.focusedCell
            ? invoke<ArrayBuffer>("get_editor_value_for_cell", {
                  cellId: shared.focusedCell,
              })
            : null;
        const namePromise = shared.focusedCell
            ? invoke<ArrayBuffer>("get_name_for_cell", {
                  cellId: shared.focusedCell,
              })
            : null;

        invoke<boolean>("is_writing").then((w) => {
            document.documentElement.classList.toggle("busy", w);
        });

        Promise.all([cellsPromise, editorPromise, namePromise]).then(
            ([cellsBuf, editorBuf, nameBuf]) => {
                // if viewport changed since we made IPC call, it means that cellsBuf is stale, and we can discard it
                if (
                    rowStart != viewportRowStart ||
                    rowEnd != viewportRowEnd ||
                    colStart != viewportColumnStart ||
                    colEnd != viewportColumnEnd
                ) {
                    return;
                }

                // "cellsBuf" length is 0 when the cells in the current viewport did not change,
                // in which case we do nothing
                if (cellsBuf.byteLength > 0) {
                    const rows = buildViewportRows(rowStart, rowEnd, colCount);
                    decodeCellsInto(
                        rows,
                        rowStart,
                        new Uint8Array(cellsBuf),
                        new DataView(cellsBuf),
                    );
                    viewportRows = rows;
                }
                if (editorBuf && !shared.isEditing) {
                    shared.editorInput = textDecoder.decode(
                        new Uint8Array(editorBuf),
                    );
                }
                if (nameBuf && !shared.isEditingCellName) {
                    shared.cellName = textDecoder.decode(
                        new Uint8Array(nameBuf),
                    );
                }
            },
            () => {},
        );

        requestAnimationFrame(renderLoop);
    }

    function startRenderLoop() {
        invoke("init_viewport").then(() => {
            updateApproximateColumnViewportBounds();
            requestAnimationFrame(renderLoop);
        });
    }

    // --- scroll handling & overlays ---

    let sos: SheetObjectsState = $state(null as any);
    let overlayRefs = $state({
        focus: null as FocusOverlay | null,
        selections: [] as (SelectionOverlay | undefined)[],
        fillOrigin: null as FillOriginOverlay | null,
        cloneSource: null as CloneSourceOverlay | null,
        refs: [] as RefOverlay[],
        tables: [] as (TableOverlay | undefined)[],
        expands: [] as (ExpandedCellOverlay | undefined)[],
    });
    let gridWrapperEl: HTMLElement | null = null;
    let clipWrapperEl: HTMLElement | null = null;
    let overlaysEl: HTMLElement | null = null;
    let scrollContainerEl: HTMLElement | null = null;

    let addRowsCount = $state(1000);
    let addColsCount = $state(50);
    let showAddRows = $state(false);
    let showAddCols = $state(false);

    function addRows() {
        const count = parseInt(String(addRowsCount), 10);
        if (!count || count < 1) return;
        const target = rowCount + count;
        expandRows(target);
        requestAnimationFrame(() => scrollToRow(target));
    }

    function addCols() {
        const count = parseInt(String(addColsCount), 10);
        if (!count || count < 1) return;
        const target = columnCount + count;
        expandColumns(target);
        requestAnimationFrame(() => scrollToColumn(target));
    }

    function getScrollContainer(): HTMLElement | null {
        if (scrollContainerEl) return scrollContainerEl;
        if (!gridWrapperEl) return null;
        scrollContainerEl =
            gridWrapperEl.querySelector<HTMLElement>("[style*='overflow']") ??
            gridWrapperEl;
        return scrollContainerEl;
    }

    export function scrollToRow(row: number) {
        const el = getScrollContainer();
        if (!el) return;
        const top = row * sizes.rowHeight;
        if (top < el.scrollTop) {
            el.scrollTop = top;
        } else if (top + sizes.rowHeight > el.scrollTop + el.clientHeight) {
            el.scrollTop = top + sizes.rowHeight - el.clientHeight;
        }
    }

    function scrollToColumn(col: number) {
        const el = getScrollContainer();
        if (!el) return;
        const leftPos = col * COL_WIDTH;
        if (leftPos < el.scrollLeft) {
            el.scrollLeft = leftPos;
        } else if (leftPos + COL_WIDTH > el.scrollLeft + el.clientWidth) {
            el.scrollLeft = leftPos + COL_WIDTH - el.clientWidth;
        }
    }

    function moveFocusBackToSpreadsheet() {
        const wrapper = gridWrapperEl?.closest<HTMLElement>(".grid-wrapper");
        if (wrapper && document.activeElement !== wrapper) wrapper.focus();
    }

    function initOverlays() {
        sos = createState(clipWrapperEl!, overlaysEl!, gridApi!);
        repositionOverlays();
    }

    export function repositionOverlays() {
        if (!gridApi || !sos) return;
        const scroller = getScrollContainer();
        reposition(sos, scroller?.scrollLeft ?? 0, scroller?.scrollTop ?? 0);
        const o = overlayRefs;
        o.focus?.reposition();
        for (const s of o.selections) s?.reposition();
        o.fillOrigin?.reposition();
        o.cloneSource?.reposition();
        for (const r of o.refs) r?.reposition();
        for (const t of o.tables) t?.reposition();
        for (const e of o.expands) e?.reposition();
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

    function handleScroll(ev: Event) {
        if ((ev.target as HTMLElement).closest(".formula-input")) return;
        const scroller = ev.target as HTMLElement;
        syncScroll(sos, scroller.scrollLeft, scroller.scrollTop);
        if (_scrolling) return;
        _scrolling = true;
        setTimeout(() => {
            _scrolling = false;
            showAddRows =
                scroller.scrollTop + scroller.clientHeight >=
                scroller.scrollHeight;
            showAddCols =
                scroller.scrollLeft + scroller.clientWidth >=
                scroller.scrollWidth;
            updateApproximateColumnViewportBounds();
            applyHeaderHighlights();
            moveFocusBackToSpreadsheet();
        }, 16);
    }

    $effect(() => {
        focusedCellBounds;
        activeSelectionBounds;
        activeFocusHasBorder;
        activeFocusHasBackground;
        selections;
        fillOriginalBounds;
        clonedFormulaBounds;
        parsedFormulaReferencesHighlights;
        shared.expandedCells;
        shared.expandModeActive;
        applyHeaderHighlights();
        repositionOverlays();
    });

    function handleRequestData(
        ev: { row: { start: number; end: number } } & { [key: string]: any },
    ): void {
        viewportRowStart = ev.row.start;
        viewportRowEnd = ev.row.end;
    }

    // --- public API ---

    export function getCell(id: CellId | null): CellData | null {
        if (!id) return null;
        const row = viewportRows[id.row - viewportRowStart];
        if (!row) return null;
        const cell = row[columnIndexToLetter(id.col)];
        if (isCellData(cell)) return cell;
        return null;
    }

    // Expanded cell keys: the map entries + the focused cell in expand mode
    let expandKeys = $derived.by(() => {
        const keys = [...shared.expandedCells.keys()];
        if (shared.expandModeActive && shared.focusedCell) {
            const fk = `${shared.focusedCell.row},${shared.focusedCell.col}`;
            if (!shared.expandedCells.has(fk)) keys.push(fk);
        }
        return keys;
    });

    export function getOverlaysEl(): HTMLElement | null {
        return overlaysEl;
    }

    function expandRows(newCount: number) {
        if (newCount <= rowCount) return;
        rowCount = newCount;
    }

    function expandColumns(newCount: number) {
        if (newCount <= columnCount) return;
        viewportColumns = buildColumns(newCount);
        columnCount = newCount;
    }

    export function saveDecorationsToJson(): string {
        const tables = shared.tables;
        const gridState = gridApi!.getState();
        const columns: any[] = gridState._columns ?? [];

        const columnWidths: Record<string, number> = {};
        for (const col of columns) {
            columnWidths[col.id] = col.width;
        }

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
        viewportRows = [];

        const el = getScrollContainer();
        if (el) el.scrollTop = 0;

        if (decorationsJson) {
            const dec = JSON.parse(decorationsJson);
            if (dec.rowCount && dec.rowCount > rowCount)
                expandRows(dec.rowCount);
            if (dec.columnCount && dec.columnCount > columnCount)
                expandColumns(dec.columnCount);
        }

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

        updateApproximateColumnViewportBounds();
        startRenderLoop();
    }

    function init(api: IApi) {
        api.intercept("open-editor", () => {
            return false;
        });

        api.intercept("close-editor", (ev: any) => {
            return false;
        });

        api.intercept("focus-cell", (ev: any) => {
            if (shared.isEditing) return false;
            if (ev?.row == null || ev?.column == null) return;
            if (ev.column === "rowNumber") {
                return false;
            }
        });

        api.on("resize-column", () => {
            requestAnimationFrame(() => repositionOverlays());
        });

        oninit?.(api);
    }

    onMount(() => {
        initOverlays();
        updateApproximateColumnViewportBounds();
        startRenderLoop();
    });
</script>

<div
    class="render-wrapper"
    bind:this={gridWrapperEl}
    onscrollcapture={handleScroll}
>
    <Grid
        bind:this={gridApi as any}
        {init}
        data={viewportRows}
        columns={viewportColumns}
        dynamic={{ rowCount, columnCount }}
        onrequestdata={handleRequestData}
        split={{ left }}
        {sizes}
        {select}
        {cellStyle}
        filterValues={{}}
    />
    {#if showAddRows}
        <div class="add-bar add-rows-bar">
            <button onclick={addRows}>Add</button>
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
        <div class="add-bar add-cols-bar">
            <button onclick={addCols}>Add</button>
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
                {#each shared.tables as table, i}
                    <TableOverlay
                        bind:this={overlayRefs.tables[i]}
                        {sos}
                        {table}
                    />
                {/each}
                {#each selections as bounds, i}
                    <SelectionOverlay
                        bind:this={overlayRefs.selections[i]}
                        {sos}
                        {bounds}
                        visible={true}
                    />
                {/each}
                <FillOriginOverlay
                    bind:this={overlayRefs.fillOrigin}
                    {sos}
                    bounds={fillOriginalBounds}
                    visible={isFilling && !!fillOriginalBounds}
                />
                <CloneSourceOverlay
                    bind:this={overlayRefs.cloneSource}
                    {sos}
                    bounds={clonedFormulaBounds}
                    visible={!!clonedFormulaBounds}
                />
                {#each parsedFormulaReferencesHighlights ?? [] as ref, i}
                    <RefOverlay
                        bind:this={overlayRefs.refs[i]}
                        {sos}
                        bounds={ref.bounds}
                        color={REF_COLORS[ref.colorIndex]}
                        active={i === activeRefIndex}
                    />
                {/each}
                {#each expandKeys as cellKey, i}
                    <ExpandedCellOverlay
                        bind:this={overlayRefs.expands[i]}
                        {sos}
                        {cellKey}
                    />
                {/each}
                <FocusOverlay
                    bind:this={overlayRefs.focus}
                    {sos}
                    bounds={activeSelectionBounds}
                    visible={!!activeSelectionBounds}
                    {isFilling}
                    showBorder={activeFocusHasBorder}
                    showBackground={activeFocusHasBackground}
                    {onfillstart}
                />
            {/if}
        </div>
    </div>
</div>

<style>
    .render-wrapper {
        width: 100%;
        height: 100%;
        position: relative;
    }

    .render-wrapper > :global(.wx-grid) {
        width: 100%;
        height: 100%;
    }

    .add-bar {
        position: absolute;
        z-index: 6;
        display: flex;
        align-items: center;
        gap: 0.5em;
        padding: 0.75em 1.5em;
        font-size: 0.8rem;
        font-family: "JetBrains Mono", monospace;
        background: var(--wx-background);
        border: var(--wx-border-medium);
        border-radius: 2px;
        box-shadow: 0 4px 16px rgba(0, 0, 0, 0.3);
    }

    .add-bar button,
    .add-bar input {
        all: unset;
        font: inherit;
        color: inherit;
        padding: 0.35em 0.6em;
        border-radius: 2px;
        background: var(--tonic-cell-tint);
        border: none;
    }

    .add-bar button {
        cursor: pointer;
        transition:
            background 100ms ease,
            border-color 100ms ease;
    }

    .add-bar button:hover {
        background: var(--tonic-cell-tint-strong);
    }

    .add-bar input {
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

    .selection-overlays-clip {
        position: absolute;
        inset: 0;
        pointer-events: none;
        z-index: 5;
    }

    .selection-overlays {
        position: absolute;
        inset: 0;
        pointer-events: none;
        will-change: transform;
    }
</style>
