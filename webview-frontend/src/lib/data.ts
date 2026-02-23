import type { IMenuOption } from "@svar-ui/svelte-menu";
import type { IColumnConfig } from "@svar-ui/svelte-grid";
import Cell from "$lib/sheet/Cell.svelte";

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
