import { createContext } from "svelte";

export type UICell = { row: number; column: string };
export type CellData = { computedValue: string; enteredText: string };

export type SheetSharedState = {
    focusedCell: UICell | undefined;
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
