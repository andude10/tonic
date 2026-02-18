import type { IMenuOption } from "@svar-ui/svelte-menu";
import type { IColumnConfig } from "@svar-ui/svelte-grid";
import CellEditor from "$lib/CellEditor.svelte";

export const menu_options: IMenuOption[] = [
    {
        id: "file",
        text: "File",
        data: [
            { id: "file-new", text: "New document", icon: "wxi wxi-file" },
            {
                id: "file-export",
                text: "Export",
                icon: "wxi wxi-download",
                data: [
                    { id: "export-pdf", text: "PDF" },
                    { id: "export-txt", text: "TXT" },
                ],
            },
            { id: "file-print", text: "Print" },
        ],
    },
    {
        id: "edit",
        text: "Edit",
        data: [
            { id: "edit-cut", text: "Cut", icon: "wxi wxi-content-cut" },
            { id: "edit-copy", text: "Copy", icon: "wxi wxi-content-copy" },
            { id: "edit-paste", text: "Paste", icon: "wxi wxi-content-paste" },
        ],
    },
    {
        id: "view",
        text: "View",
        data: [
            { id: "view-fullscreen", text: "Fullscreen" },
            { id: "view-option", text: "..." },
        ],
    },
    // more menu items
];

export const baseColumns: IColumnConfig[] = (() => {
    const columns: IColumnConfig[] = [
        { id: "rowNumber", width: 50, resize: true },
    ];
    for (let i = 0; i < 26; i++) {
        const id = String.fromCharCode(65 + i);
        columns.push({
            id,
            header: id,
            cell: CellEditor,
            width: 100,
            resize: true,
        });
    }
    return columns;
})();

export const baseRows = Array.from({ length: 1000 }, (_, i) => {
    const row: any = { id: i + 1, rowNumber: i + 1 };
    for (let j = 0; j < 26; j++) {
        row[String.fromCharCode(65 + j)] = "";
    }
    return row;
});

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
