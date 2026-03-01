import { columnIndexToLetter, type CellId } from "./shared";
import "./overlays.css";

export type CellBounds = {
    minR: number;
    maxR: number;
    minC: number;
    maxC: number;
};

export type Rect = {
    left: number;
    top: number;
    width: number;
    height: number;
};

export type SheetObject = {
    el: HTMLElement;
    bounds: CellBounds;
    visible: boolean;
};

export type SheetObjectsState = {
    container: HTMLElement;
    clipWrapper: HTMLElement;
    baseScrollLeft: number;
    baseScrollTop: number;
    // per-column cache, rebuilt on reposition()
    colIds: string[];
    colLefts: number[];
    colRights: number[];
    rowHeight: number;
    headerHeight: number;
    rowNumWidth: number;
};

export function createState(
    clipWrapper: HTMLElement,
    container: HTMLElement,
): SheetObjectsState {
    return {
        container,
        clipWrapper,
        baseScrollLeft: 0,
        baseScrollTop: 0,
        colIds: [],
        colLefts: [],
        colRights: [],
        rowHeight: 37,
        headerHeight: 37,
        rowNumWidth: 50,
    };
}

export function createObject(
    state: SheetObjectsState,
    className: string,
): SheetObject {
    const el = document.createElement("div");
    el.className = className;
    el.style.display = "none";
    state.container.appendChild(el);
    return {
        el,
        bounds: { minR: 0, maxR: 0, minC: 0, maxC: 0 },
        visible: false,
    };
}

export function destroyObject(obj: SheetObject): void {
    obj.el.remove();
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

/** Reset scroll baseline, rebuild column cache, update clip mask. */
export function reposition(
    state: SheetObjectsState,
    gridState: any,
    scrollLeft: number,
    scrollTop: number,
): void {
    state.baseScrollLeft = scrollLeft;
    state.baseScrollTop = scrollTop;
    state.container.style.transform = "translate(0px, 0px)";

    const columns: any[] = gridState._columns ?? [];
    state.rowHeight = gridState._sizes?.rowHeight ?? 37;
    state.headerHeight = gridState._sizes?.headerHeight ?? 37;

    const rowNumCol = columns.find((c: any) => c.id === "rowNumber");
    state.rowNumWidth = rowNumCol ? (rowNumCol.width ?? 50) : 50;
    state.clipWrapper.style.clipPath = `inset(${state.headerHeight}px 0 0 ${state.rowNumWidth}px)`;

    state.colIds = [];
    state.colLefts = [];
    state.colRights = [];
    let x = 0;
    for (const col of columns) {
        const w = col.width ?? 100;
        state.colIds.push(col.id);
        state.colLefts.push(x);
        state.colRights.push(x + w);
        x += w;
    }
}

/** Convert cell bounds to pixel rect relative to scroll baseline. */
export function getRect(
    state: SheetObjectsState,
    bounds: CellBounds,
): Rect | null {
    const minLetter = columnIndexToLetter(bounds.minC);
    const maxLetter = columnIndexToLetter(bounds.maxC);

    let minColLeft = -1;
    let maxColRight = -1;
    for (let i = 0; i < state.colIds.length; i++) {
        if (state.colIds[i] === minLetter) minColLeft = state.colLefts[i];
        if (state.colIds[i] === maxLetter) maxColRight = state.colRights[i];
    }
    if (minColLeft < 0 || maxColRight < 0) return null;

    const left = minColLeft - state.baseScrollLeft;
    const top =
        state.headerHeight +
        bounds.minR * state.rowHeight -
        state.baseScrollTop;
    return {
        left,
        top,
        width: maxColRight - state.baseScrollLeft - left,
        height: (bounds.maxR + 1 - bounds.minR) * state.rowHeight,
    };
}

/** Position a SheetObject's element to match its bounds. */
export function applyRect(state: SheetObjectsState, obj: SheetObject): void {
    if (!obj.visible) {
        obj.el.style.display = "none";
        return;
    }
    const rect = getRect(state, obj.bounds);
    if (!rect) {
        obj.el.style.display = "none";
        return;
    }
    obj.el.style.display = "block";
    obj.el.style.left = `${rect.left}px`;
    obj.el.style.top = `${rect.top}px`;
    obj.el.style.width = `${rect.width}px`;
    obj.el.style.height = `${rect.height}px`;
}

/** All cells within bounds. */
export function cellsInBounds(bounds: CellBounds): CellId[] {
    const cells: CellId[] = [];
    for (let r = bounds.minR; r <= bounds.maxR; r++) {
        for (let c = bounds.minC; c <= bounds.maxC; c++) {
            cells.push({ row: r, col: c });
        }
    }
    return cells;
}

/** Cells in `a` that are NOT in `b`. */
export function cellsOutside(a: CellBounds, b: CellBounds): CellId[] {
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

/** Hit test: is a point (in scroll-container-relative coords) inside this SheetObject? */
export function hitTest(
    state: SheetObjectsState,
    obj: SheetObject,
    x: number,
    y: number,
): boolean {
    if (!obj.visible) return false;
    const rect = getRect(state, obj.bounds);
    if (!rect) return false;
    return (
        x >= rect.left &&
        x <= rect.left + rect.width &&
        y >= rect.top &&
        y <= rect.top + rect.height
    );
}
