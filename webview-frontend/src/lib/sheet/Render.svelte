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
        type InvalidateFrotnendPayload,
        type SheetRow,
    } from "$lib/sheet/shared";
    import { invoke } from "@tauri-apps/api/core";
    import { listen } from "@tauri-apps/api/event";
    import { endTimer, startTimer } from "$lib/stats.svelte";
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
    const CELLS_CACHE_ROWS = 500;
    const CELLS_CACHE_COLUMNS = 50;
    const INITIAL_FETCH_ROW_END = 40;
    const CELLS_CACHE_FETCH_THROTTLE_MS = 100;
    const SCROLL_VISUAL_THROTTLE_MS = 16;

    const textDecoder = new TextDecoder();
    const EMPTY_BODY = new Uint8Array();

    let left = 1;
    let select = false;
    let sizes = { headerHeight: 28, rowHeight: 28 };

    // --- data model ---
    // viewportRows and viewportColumns are the only data the Grid sees.
    // Display rows are sliced from the cells cache below.

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

    // build empty rows; backend payload fills display cells
    function buildViewportRows(
        start: number,
        end: number,
        colStart = 0,
        colEnd = columnCount - 1,
    ): SheetRow[] {
        const count = end - start + 1;
        const rows: SheetRow[] = new Array(count);
        for (let i = 0; i < count; i++) {
            const id = start + i + 1; // 1-indexed for SVAR Grid
            const row = { id, rowNumber: id } as SheetRow;
            for (let c = colStart; c <= colEnd; c++) {
                const colId = columnIndexToLetter(c);
                row[colId] = {
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

    // --- display cells ---

    let viewportColumnStart = 0;
    let viewportColumnEnd = 25;
    let viewportRowStart = 0;
    let viewportRowEnd = -1;
    let cachedRows: SheetRow[] = [];
    let cachedRowStart = 0;
    let cachedRowEnd = -1;
    let cachedColumnStart = 0;
    let cachedColumnEnd = -1;

    function updateApproximateColumnViewportBounds() {
        const el = getScrollContainer();
        if (!el || !el.clientWidth) return;
        viewportColumnStart = (el.scrollLeft / COL_WIDTH) | 0;
        viewportColumnEnd = Math.min(
            Math.ceil((el.scrollLeft + el.clientWidth) / COL_WIDTH),
            columnCount - 1,
        );
    }

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

            // payload rows are absolute; cache rows start at rowStart
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

    function cellsCacheBounds(
        start: number,
        end: number,
        size: number,
        limit: number,
    ) {
        const count = Math.max(size, end - start + 1);
        const before = ((count - (end - start + 1)) / 2) | 0;
        let cacheStart = Math.max(0, start - before);
        let cacheEnd = Math.min(limit - 1, cacheStart + count - 1);
        cacheStart = Math.max(0, cacheEnd - count + 1);
        return { start: cacheStart, end: cacheEnd };
    }

    function updateFocusedCellInfo() {
        const cell = shared.focusedCell;
        if (!cell) return;

        Promise.all([
            invoke<ArrayBuffer>("get_editor_value_for_cell", {
                cellId: cell,
            }),
            invoke<ArrayBuffer>("get_name_for_cell", {
                cellId: cell,
            }),
        ]).then(
            ([editorBuf, nameBuf]) => {
                if (
                    !shared.focusedCell ||
                    shared.focusedCell.row !== cell.row ||
                    shared.focusedCell.col !== cell.col
                ) {
                    return;
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
    }

    async function fetchCellsCache(
        visibleRowStart: number,
        visibleRowEnd: number,
    ) {
        if (rowCount <= 0 || columnCount <= 0) {
            cachedRows = [];
            cachedRowEnd = -1;
            viewportRows = [];
            return;
        }
        const rowBounds = cellsCacheBounds(
            visibleRowStart,
            visibleRowEnd,
            CELLS_CACHE_ROWS,
            rowCount,
        );
        const colBounds = cellsCacheBounds(
            viewportColumnStart,
            viewportColumnEnd,
            CELLS_CACHE_COLUMNS,
            columnCount,
        );
        const rowStart = rowBounds.start;
        const rowEnd = rowBounds.end;
        const colStart = colBounds.start;
        const colEnd = colBounds.end;

        startTimer("get_display_cells", true);
        const cellsBuf = await invoke<ArrayBuffer>(
            "get_display_cells",
            EMPTY_BODY,
            {
                headers: {
                    "row-start": String(rowStart),
                    "row-end": String(rowEnd),
                    "col-start": String(colStart),
                    "col-end": String(colEnd),
                },
            },
        ).finally(() => endTimer("get_display_cells"));

        if (cellsBuf.byteLength === 0) return;

        const currentStart =
            viewportRowEnd >= viewportRowStart
                ? viewportRowStart
                : visibleRowStart;
        const currentEnd =
            viewportRowEnd >= viewportRowStart ? viewportRowEnd : visibleRowEnd;

        // if user scrolled away while IPC was in flight, ignore this cells cache
        if (
            currentStart < rowStart ||
            currentEnd > rowEnd ||
            viewportColumnStart < colStart ||
            viewportColumnEnd > colEnd
        ) {
            return;
        }

        const rows = buildViewportRows(rowStart, rowEnd, colStart, colEnd);
        decodeCellsInto(
            rows,
            rowStart,
            new Uint8Array(cellsBuf),
            new DataView(cellsBuf),
        );
        cachedRows = rows;
        cachedRowStart = rowStart;
        cachedRowEnd = rowEnd;
        cachedColumnStart = colStart;
        cachedColumnEnd = colEnd;
        viewportRows = cachedRows.slice(
            currentStart - cachedRowStart,
            currentEnd - cachedRowStart + 1,
        );
    }

    // --- scroll handling & overlays ---

    let scrollVisualTimer: ReturnType<typeof setTimeout> | null = null;

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

    function handleRequestData(ev: { row: { start: number; end: number } }) {
        viewportRowStart = ev.row.start;
        viewportRowEnd = ev.row.end;

        if (
            cachedRows.length > 0 &&
            viewportRowStart >= cachedRowStart &&
            viewportRowEnd <= cachedRowEnd &&
            viewportColumnStart >= cachedColumnStart &&
            viewportColumnEnd <= cachedColumnEnd
        ) {
            if (
                !viewportRows.length ||
                viewportRows[0].id !== viewportRowStart + 1 ||
                viewportRows[viewportRows.length - 1].id !== viewportRowEnd + 1
            ) {
                // if visible cells are inside cache, just take a slice
                viewportRows = cachedRows.slice(
                    viewportRowStart - cachedRowStart,
                    viewportRowEnd - cachedRowStart + 1,
                );
            }
        } else {
            fetchCellsCache(viewportRowStart, viewportRowEnd);
        }
    }

    function handleScroll(ev: Event) {
        if ((ev.target as HTMLElement).closest(".formula-input")) return;
        const scroller = ev.target as HTMLElement;
        syncScroll(sos, scroller.scrollLeft, scroller.scrollTop);
        updateApproximateColumnViewportBounds();

        if (
            viewportColumnStart < cachedColumnStart ||
            viewportColumnEnd > cachedColumnEnd
        ) {
            handleRequestData({
                row: { start: viewportRowStart, end: viewportRowEnd + 1 },
            });
        }

        if (scrollVisualTimer) return;
        scrollVisualTimer = setTimeout(() => {
            scrollVisualTimer = null;
            showAddRows =
                scroller.scrollTop + scroller.clientHeight >=
                scroller.scrollHeight;
            showAddCols =
                scroller.scrollLeft + scroller.clientWidth >=
                scroller.scrollWidth;
            applyHeaderHighlights();
            moveFocusBackToSpreadsheet();
        }, SCROLL_VISUAL_THROTTLE_MS);
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

    $effect(() => {
        shared.focusedCell;
        updateFocusedCellInfo();
    });

    // --- public API ---

    export function getCell(id: CellId | null): CellData | null {
        if (!id) return null;
        if (!viewportRows.length) return null;
        const row = viewportRows[id.row - ((viewportRows[0].id as number) - 1)];
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
        cachedRows = [];
        viewportRowStart = 0;
        viewportRowEnd = -1;
        cachedRowStart = 0;
        cachedRowEnd = -1;
        cachedColumnStart = 0;
        cachedColumnEnd = -1;

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
        fetchCellsCache(0, Math.min(INITIAL_FETCH_ROW_END, rowCount - 1));
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
        const unlistenInvalidation = listen<InvalidateFrotnendPayload>(
            "invalidate-frontend",
            (ev) => {
                if (!ev.payload.viewport) return;
                // backend changed display cells, so cells cache is stale
                cachedRows = [];
                updateFocusedCellInfo();
                fetchCellsCache(
                    viewportRowEnd >= viewportRowStart ? viewportRowStart : 0,
                    viewportRowEnd >= viewportRowStart
                        ? viewportRowEnd
                        : Math.min(INITIAL_FETCH_ROW_END, rowCount - 1),
                );
            },
        );
        initOverlays();
        updateApproximateColumnViewportBounds();
        fetchCellsCache(0, Math.min(INITIAL_FETCH_ROW_END, rowCount - 1));
        return () => {
            unlistenInvalidation.then((fn) => fn());
        };
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

    .render-wrapper :global(.wx-scroll),
    .render-wrapper :global(.wx-body),
    .render-wrapper :global(.wx-data),
    .render-wrapper :global(.wx-row) {
        overflow-anchor: none;
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
