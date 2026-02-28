import { createContext } from "svelte";

/** User-facing cell ID: letter column, 1-indexed row. Only at SVAR grid boundary. */
export type UICell = { row: number; column: string };
/** Internal cell ID: 0-indexed row and col. */
export type CellId = { row: number; col: number };
export type CellData = { computedValue: string; enteredText: string };

export type SheetSharedState = {
    focusedCell: CellId | undefined;
    isEditing: boolean;
    readonly editorInputIsFormula: boolean;
    editorInput: string;
    readonly editorInputHtml: string;
    caretPosition: number;
    commitEdit(): void;
};

export const [getSheetSharedState, setSheetSharedState] =
    createContext<SheetSharedState>();

export type SheetRow = {
    id: number;
    rowNumber: number;
} & Record<string, CellData | number>;

/** "A" → 0, "B" → 1, "Z" → 25, "AA" → 26 */
export function columnLetterToIndex(id: string): number {
    let n = 0;
    for (let i = 0; i < id.length; i++) {
        n = n * 26 + (id.charCodeAt(i) - 64);
    }
    return n - 1;
}

/** 0 → "A", 1 → "B", 25 → "Z", 26 → "AA" */
export function columnIndexToLetter(index: number): string {
    let s = "";
    let n = index + 1;
    while (n > 0) {
        n--;
        s = String.fromCharCode(65 + (n % 26)) + s;
        n = Math.floor(n / 26);
    }
    return s;
}

export function isCellData(val: CellData | number): val is CellData {
    return (
        typeof val === "object" &&
        "computedValue" in val &&
        "enteredText" in val
    );
}

// // todo: infinite scroll
// export function generateRows(rows: any[], rangeDifference: number): void {
//     const clearRow = (row: any) => {
//         for (let j = 0; j < 26; j++) {
//             row[String.fromCharCode(65 + j)] = "";
//         }
//     };

//     // if scorlling up (rangeDifference < 0), then empty top rows
//     if (rangeDifference < 0) {
//         const count = Math.abs(rangeDifference);
//         for (let i = 0; i < count && i < rows.length; i++) {
//             clearRow(rows[i]);
//         }
//     } else if (rangeDifference > 0) {
//         // otherwise, empty last rows
//         const count = rangeDifference;
//         for (let i = rows.length - count; i < rows.length && i >= 0; i++) {
//             clearRow(rows[i]);
//         }
//     }
// }
