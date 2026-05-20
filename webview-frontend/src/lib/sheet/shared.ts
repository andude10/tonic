import { createContext } from "svelte";

export const REF_COLORS = [
    "#d98c5f",
    "#6aab7b",
    "#c4a24d",
    "#9a7ec8",
    "#5ba8a8",
];

/** User-facing cell ID: letter column, 1-indexed row. Only at SVAR grid boundary. */
export type UICell = { row: number; column: string };
/** Internal cell ID: 0-indexed row and col. */
export type CellId = { row: number; col: number };
export type CellFormatting = {
    bold?: boolean;
    italic?: boolean;
    strikethrough?: boolean;
    textColor?: string | null;
};

export type CellData = {
    computedValue: string;
    isFormula: boolean;
    formatting: CellFormatting;
    isPending?: boolean;
    isError?: boolean;
    errorMessage?: string;
};
export type ChangeBounds = {
    min_row: number;
    max_row: number;
    min_col: number;
    max_col: number;
};

export type CellRange = {
    minR: number;
    maxR: number;
    minC: number;
    maxC: number;
};

export type FilterOption = { id: number; val: string; selected: boolean };

export type InvalidateFrotnendPayload = {
    viewport?: true;
    file_name?: string | null;
    file_path?: string | null;
};

export type TableData = {
    id: number;
    headerBounds: CellRange;
    bodyBounds: CellRange;
    title: string;
    hasProjection: boolean;
    hiddenRowsCount: number;
};

export type CellFormattingData = { extraWidth: number; extraHeight: number };

export type SheetSharedState = {
    focusedCell: CellId | null;
    hoveredCell: CellId | null;
    selections: CellRange[];
    cellName: string;
    isEditing: boolean;
    isEditingCellName: boolean;
    readonly editorInputIsFormula: boolean;
    editorInput: string;
    readonly editorInputHtml: string;
    caretPosition: number;
    editorInputWidth: number;
    tables: TableData[];
    readonly cellStyles: Map<string, string>;
    expandedCells: Map<string, CellFormattingData>;
    expandModeActive: boolean;
    getCellDisplayValue(row: number, col: number): string;
    getCellIsFormula(row: number, col: number): boolean;
    getCellFormatting(row: number, col: number): CellFormatting;
    commitEdit(): void;
};

export const [getSheetSharedState, setSheetSharedState] =
    createContext<SheetSharedState>();

export type SheetRow = {
    id: number;
    rowNumber: number;
} & Record<string, CellData | number>;

const LETTERS = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";

/** "A" -> 0, "B" -> 1, "Z" -> 25, "AA" -> 26 */
export function columnLetterToIndex(id: string): number {
    if (id.length === 1) return id.charCodeAt(0) - 65;
    let n = 0;
    for (let i = 0; i < id.length; i++) {
        n = n * 26 + (id.charCodeAt(i) - 64);
    }
    return n - 1;
}

/** 0 -> "A", 1 -> "B", 25 -> "Z", 26 -> "AA" */
export function columnIndexToLetter(index: number): string {
    if (index < 26) return LETTERS[index];
    let s = "";
    let n = index + 1;
    while (n > 0) {
        n--;
        s = String.fromCharCode(65 + (n % 26)) + s;
        n = Math.floor(n / 26);
    }
    return s;
}

// todo: remove, indexing is messy again
export function parseSvarID(
    raw: string | undefined,
): string | number | undefined {
    if (!raw) return raw;
    if (raw.startsWith(":")) return raw.substring(1);
    const n = Number(raw);
    if (!isNaN(n)) return n;
    return raw;
}

export function isCellData(val: CellData | number): val is CellData {
    return (
        typeof val === "object" && "computedValue" in val && "isFormula" in val
    );
}
