import type { CellId } from "../shared";

export type CellRange = {
    minR: number;
    maxR: number;
    minC: number;
    maxC: number;
};

export type PixelRect = {
    left: number;
    top: number;
    width: number;
    height: number;
};

export type SheetObjectsState = {
    container: HTMLElement;
    clipWrapper: HTMLElement;
    gridApi: any;
    baseScrollLeft: number;
    baseScrollTop: number;
    rowHeight: number;
    headerHeight: number;
    rowNumWidth: number;
    /** Prefix-sum of column left pixel edges, indexed by render position. */
    columnEdges: number[];
};

export function createState(
    clipWrapper: HTMLElement,
    container: HTMLElement,
    gridApi: any,
): SheetObjectsState {
    return {
        container,
        clipWrapper,
        gridApi,
        baseScrollLeft: 0,
        baseScrollTop: 0,
        rowHeight: 37,
        headerHeight: 37,
        rowNumWidth: 50,
        columnEdges: [],
    };
}

/** Apply CSS transform to follow scroll without repositioning. */
export function syncScroll(
    state: SheetObjectsState,
    scrollLeft: number,
    scrollTop: number,
): void {
    const dx = state.baseScrollLeft - scrollLeft;
    const dy = state.baseScrollTop - scrollTop;
    state.container.style.transform = `translate(${dx}px, ${dy}px)`;
}

/** Reset scroll baseline and update clip mask. */
export function reposition(
    state: SheetObjectsState,
    scrollLeft: number,
    scrollTop: number,
): void {
    state.baseScrollLeft = scrollLeft;
    state.baseScrollTop = scrollTop;
    state.container.style.transform = "translate(0px, 0px)";

    const gridState = state.gridApi.getState();
    const columns: any[] = gridState._columns ?? [];
    state.rowHeight = gridState._sizes?.rowHeight ?? 37;
    state.headerHeight = gridState._sizes?.headerHeight ?? 37;

    const rowNumCol = columns.find((c: any) => c.id === "rowNumber");
    state.rowNumWidth = rowNumCol ? (rowNumCol.width ?? 50) : 50;
    state.clipWrapper.style.clipPath = `inset(${state.headerHeight}px 0 0 ${state.rowNumWidth}px)`;

    // rebuild column edge cache (prefix-sum of widths)
    const edges = state.columnEdges;
    edges.length = columns.length + 1;
    let x = 0;
    for (let i = 0; i < columns.length; i++) {
        edges[i] = x;
        x += columns[i].width ?? 100;
    }
    edges[columns.length] = x;
}

/** Convert a cell range to pixel coordinates relative to scroll baseline. */
export function cellRangeToPixels(
    state: SheetObjectsState,
    bounds: CellRange,
): PixelRect | null {
    const edges = state.columnEdges;
    if (edges.length === 0) return null;

    // +1 because render column 0 is "rowNumber"
    const minIdx = bounds.minC + 1;
    const maxIdx = bounds.maxC + 1;
    if (maxIdx + 1 >= edges.length) return null;

    const minLeft = edges[minIdx];
    const maxRight = edges[maxIdx + 1];

    const left = minLeft - state.baseScrollLeft;
    const top =
        state.headerHeight +
        bounds.minR * state.rowHeight -
        state.baseScrollTop;
    return {
        left,
        top,
        width: maxRight - state.baseScrollLeft - left,
        height: (bounds.maxR + 1 - bounds.minR) * state.rowHeight,
    };
}

/** All cells within bounds. */
export function cellsInBounds(bounds: CellRange): CellId[] {
    const cells: CellId[] = [];
    for (let r = bounds.minR; r <= bounds.maxR; r++) {
        for (let c = bounds.minC; c <= bounds.maxC; c++) {
            cells.push({ row: r, col: c });
        }
    }
    return cells;
}

/** Cells in `a` that are NOT in `b`. */
export function cellsOutside(a: CellRange, b: CellRange): CellId[] {
    const cells: CellId[] = [];
    for (let r = a.minR; r <= a.maxR; r++) {
        for (let c = a.minC; c <= a.maxC; c++) {
            if (r >= b.minR && r <= b.maxR && c >= b.minC && c <= b.maxC)
                continue;
            cells.push({ row: r, col: c });
        }
    }
    return cells;
}
