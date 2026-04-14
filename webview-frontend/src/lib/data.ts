import type { IMenuOption } from "@svar-ui/svelte-menu";

export const menu_options: IMenuOption[] = [
    {
        id: "file",
        text: "File",
        data: [
            { id: "file-new", text: "New", icon: "wxi wxi-file" },
            { id: "file-open", text: "Open...", icon: "wxi wxi-folder" },
            {
                id: "file-save",
                text: "Save",
                subtext: "Ctrl+S",
                icon: "wxi wxi-download",
            },
            {
                id: "file-save-as",
                text: "Save As...",
                subtext: "Ctrl+Shift+S",
                icon: "wxi wxi-download",
            },
        ],
    },
    {
        id: "view",
        text: "View",
        data: [
            {
                id: "view-theme",
                text: "Theme",
                data: [
                    { id: "view-theme-dark", text: "Dark" },
                    { id: "view-theme-light", text: "Light" },
                ],
            },
        ],
    },
    {
        id: "help",
        text: "Help",
        data: [{ id: "help-shortcuts", text: "Shortcuts" }],
    },
];
