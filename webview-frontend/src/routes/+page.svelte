<script lang="ts">
    import { WillowDark } from "@svar-ui/svelte-grid";
    import { Globals } from "@svar-ui/svelte-core";
    import { MenuBar, type IMenuOption } from "@svar-ui/svelte-menu";
    import WindowBar from "$lib/WindowBar.svelte";
    import Sheet from "$lib/sheet/Sheet.svelte";
    import type { InvalidateFrotnendPayload } from "$lib/sheet/shared";
    import BottomBar from "$lib/BottomBar.svelte";
    import ShowDependencyGraph from "$lib/side-panels/ShowDependencyGraph.svelte";
    import Extensions from "$lib/side-panels/Extensions.svelte";
    import Shortcuts from "$lib/side-panels/Shortcuts.svelte";
    import { trackFps } from "$lib/stats.svelte";
    import { showError } from "$lib/notice";
    import type { IApi } from "@svar-ui/svelte-grid";
    import { attachConsole } from "@tauri-apps/plugin-log";
    import { getCurrentWindow } from "@tauri-apps/api/window";
    import { invoke } from "@tauri-apps/api/core";
    import { open, save } from "@tauri-apps/plugin-dialog";
    import { listen } from "@tauri-apps/api/event";
    import { onMount } from "svelte";

    import {
        initExtensionDispatcher,
        loadAllExtensions,
    } from "$lib/extensions/dispatcher";

    attachConsole();
    trackFps();
    initExtensionDispatcher();

    let sheet: Sheet;
    let theme: "dark" | "light" = $state("dark");
    let currentFilePath: string | null = $state(null);
    let fileTitle: string | null = $state(null);
    let dialogOpen = $state(false);
    let isSaved = $state(true);
    let isSaving = $state(false);
    let isLoading = $state(false);
    let activePanel: string | null = $state(null);

    function togglePanel(id: string) {
        activePanel = activePanel === id ? null : id;
    }

    const DIALOG_FILTER = { name: "Tonic Spreadsheet", extensions: ["tcs"] };

    const menu_options: IMenuOption[] = [
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

    async function commitSaveAs() {
        dialogOpen = true;
        const path = await save({ filters: [DIALOG_FILTER] });
        dialogOpen = false;
        if (!path) return;
        isSaving = true;
        const uiDecorationsJson = sheet.saveDecorationsToJson();
        await invoke("save_file", { path, uiDecorationsJson });
        isSaving = false;
    }

    async function commitSave() {
        if (currentFilePath) {
            isSaving = true;
            const uiDecorationsJson = sheet.saveDecorationsToJson();
            await invoke("save_file", {
                path: currentFilePath,
                uiDecorationsJson,
            });
            isSaving = false;
        } else {
            await commitSaveAs();
        }
    }

    async function commitOpen() {
        dialogOpen = true;
        let path: string | null = null;
        try {
            const selected = await open({
                multiple: false,
                directory: false,
                filters: [DIALOG_FILTER],
            });
            if (typeof selected === "string") path = selected;
        } catch (e) {
            showError("Open dialog failed: " + e);
        } finally {
            dialogOpen = false;
        }
        if (!path) return;

        isLoading = true;
        try {
            const decorationsJson = await invoke<string>("open_file", { path });
            sheet.onFileLoad(decorationsJson);
            await loadAllExtensions();
        } catch (e) {
            showError(`Failed to open file '${path}`);
        } finally {
            isLoading = false;
        }
    }

    // add check icon to active theme item
    const active_menu = $derived(
        menu_options.map((item) =>
            item.id !== "view"
                ? item
                : {
                      ...item,
                      data: item.data?.map((sub) =>
                          sub.id !== "view-theme"
                              ? sub
                              : {
                                    ...sub,
                                    data: sub.data?.map((t) => ({
                                        ...t,
                                        icon:
                                            t.id === `view-theme-${theme}`
                                                ? "wxi wxi-check"
                                                : "",
                                    })),
                                },
                      ),
                  },
        ),
    );

    // portaled menus render outside .layout-container, so keep their css vars in sync with theme
    $effect(() => {
        const r = document.documentElement;
        if (theme === "dark") {
            r.style.setProperty("--tonic-popup-bg", "#1e2024");
            r.style.setProperty("--tonic-popup-border-color", "#44464c");
            r.style.setProperty("--tonic-popup-font", "rgba(255,255,255,0.9)");
            r.style.setProperty("--tonic-icon-color", "#c0c0c0");
            r.style.setProperty("--tonic-scrollbar", "#3c3e44");
            r.style.setProperty("--tonic-scrollbar-hover", "#52545a");
        } else {
            r.style.setProperty("--tonic-popup-bg", "#ffffff");
            r.style.setProperty("--tonic-popup-border-color", "#c8c9cc");
            r.style.setProperty("--tonic-popup-font", "#242529");
            r.style.setProperty("--tonic-icon-color", "#58585a");
            r.style.setProperty("--tonic-scrollbar", "#b8b9bc");
            r.style.setProperty("--tonic-scrollbar-hover", "#9a9b9e");
        }
    });

    async function onMenuClick(ev: any) {
        const id = ev.action?.id;
        if (!id) return;
        switch (id) {
            case "file-new":
                await invoke("new_file");
                sheet.onFileLoad();
                break;
            case "file-open":
                await commitOpen();
                break;
            case "file-save":
                await commitSave();
                break;
            case "file-save-as":
                await commitSaveAs();
                break;
            case "view-show-dependency-graph":
                togglePanel("graph");
                break;
            case "view-extensions":
                togglePanel("extensions");
                break;
            case "help-shortcuts":
                togglePanel("shortcuts");
                break;
            case "view-theme-dark":
                theme = "dark";
                break;
            case "view-theme-light":
                theme = "light";
                break;
        }
    }

    async function onTitleChange(e: Event) {
        const el = e.target as HTMLElement;
        const newName = (el.textContent ?? "").trim();
        if (!newName || newName === fileTitle) {
            el.textContent = fileTitle ?? "";
            return;
        }
        const finalName = newName.endsWith(".tcs") ? newName : newName + ".tcs";
        try {
            await invoke("rename_current_file", { newName: finalName });
            fileTitle = finalName;
            el.textContent = finalName;
        } catch (e) {
            showError("Rename failed: " + e);
            el.textContent = fileTitle ?? "";
        }
    }

    function onTitleKeyDown(e: KeyboardEvent) {
        if (e.key === "Enter") {
            e.preventDefault();
            (e.target as HTMLElement).blur();
        }
    }

    function handleKeyDown(e: KeyboardEvent) {
        if (e.ctrlKey && e.shiftKey && e.key === "S") {
            e.preventDefault();
            commitSaveAs();
        } else if (e.ctrlKey && e.key === "s") {
            e.preventDefault();
            commitSave();
        }
    }

    onMount(async () => {
        listen<boolean>("save-status", (ev) => {
            isSaved = ev.payload;
        });
        listen<InvalidateFrotnendPayload>("invalidate-frontend", (ev) => {
            if (ev.payload.file_name !== undefined)
                fileTitle = ev.payload.file_name;
            if (ev.payload.file_path !== undefined)
                currentFilePath = ev.payload.file_path;
        });

        const [name, path] =
            await invoke<[string | null, string | null]>("get_file_info");
        currentFilePath = path;
        fileTitle = name;
        if (!currentFilePath) {
            await invoke("new_file");
        }
        await loadAllExtensions();

        // on start-up, window flashes white screen before rendering
        // it is known webview issue: https://github.com/tauri-apps/tauri/issues/1564
        // this allows to show window once everything is loaded
        await getCurrentWindow().show();
    });
</script>

<div class="root noselect" onkeydowncapture={handleKeyDown}>
    <WillowDark>
        <Globals>
            <div
                class="layout-container"
                data-theme={theme}
                data-wx-portal-root="true"
            >
                <WindowBar>
                    <span class="file-title">
                        <span
                            contenteditable="true"
                            role="textbox"
                            tabindex="0"
                            style="outline: none"
                            onblur={onTitleChange}
                            onkeydown={onTitleKeyDown}>{fileTitle}</span
                        >
                        {#if isSaving}
                            <svg
                                class="file-status save-icon spinning"
                                viewBox="0 0 16 16"
                                xmlns="http://www.w3.org/2000/svg"
                                fill="none"
                            >
                                <g
                                    fill="#4184BF"
                                    fill-rule="evenodd"
                                    clip-rule="evenodd"
                                >
                                    <path
                                        d="M8 1.5a6.5 6.5 0 100 13 6.5 6.5 0 000-13zM0 8a8 8 0 1116 0A8 8 0 010 8z"
                                        opacity=".2"
                                    />
                                    <path
                                        d="M7.25.75A.75.75 0 018 0a8 8 0 018 8 .75.75 0 01-1.5 0A6.5 6.5 0 008 1.5a.75.75 0 01-.75-.75z"
                                    />
                                </g>
                            </svg>
                        {:else if isSaved}
                            <svg
                                class="file-status save-icon"
                                viewBox="0 0 60 60"
                                xmlns="http://www.w3.org/2000/svg"
                            >
                                <title>All changes are saved</title>
                                <path
                                    fill="#08A64D"
                                    fill-rule="evenodd"
                                    d="M30 0a30 30 0 110 60 30 30 0 010-60zm-16.986 36.765a3.484 3.484 0 010-4.9l1.766-1.756a3.185 3.185 0 014.574.051l3.12 3.237a1.592 1.592 0 002.311 0l15.9-16.39a3.187 3.187 0 014.6-.027L47 18.714a3.482 3.482 0 010 4.846l-21.109 21.451a3.185 3.185 0 01-4.552.03z"
                                />
                            </svg>
                        {:else}
                            <svg
                                class="file-status save-icon"
                                viewBox="0 0 60 60"
                                xmlns="http://www.w3.org/2000/svg"
                            >
                                <title>Changes are not saved</title>
                                <path
                                    fill="#ef4444"
                                    fill-rule="evenodd"
                                    d="M940,510a30,30,0,1,1,30-30A30,30,0,0,1,940,510Zm15-20.047A3.408,3.408,0,0,1,955,494.77l-0.221.22a3.42,3.42,0,0,1-4.833,0l-8.764-8.755a1.71,1.71,0,0,0-2.417,0l-8.741,8.747a3.419,3.419,0,0,1-4.836,0l-0.194-.193a3.408,3.408,0,0,1,.017-4.842l8.834-8.735a1.7,1.7,0,0,0,0-2.43l-8.831-8.725a3.409,3.409,0,0,1-.018-4.844l0.193-.193a3.413,3.413,0,0,1,2.418-1c0.944,0,3.255,1.835,3.872,2.455l7.286,7.287a1.708,1.708,0,0,0,2.417,0l8.764-8.748a3.419,3.419,0,0,1,4.832,0L955,465.243a3.408,3.408,0,0,1,0,4.818l-8.727,8.737a1.7,1.7,0,0,0,0,2.407Z"
                                    transform="translate(-910 -450)"
                                />
                            </svg>
                        {/if}
                    </span>
                    <MenuBar options={active_menu} onclick={onMenuClick}
                    ></MenuBar>
                </WindowBar>

                <Sheet bind:this={sheet}>
                    {#snippet sidebar()}
                        {#if activePanel}
                            <div class="side-panel-popup">
                                {#if activePanel === "graph"}
                                    <ShowDependencyGraph
                                        onclose={() => (activePanel = null)}
                                    />
                                {:else if activePanel === "extensions"}
                                    <Extensions
                                        onclose={() => (activePanel = null)}
                                    />
                                {:else if activePanel === "shortcuts"}
                                    <Shortcuts
                                        onclose={() => (activePanel = null)}
                                    />
                                {/if}
                            </div>
                        {/if}
                    {/snippet}
                </Sheet>
                {#if dialogOpen}<div class="dialog-overlay"></div>{/if}
                {#if isLoading}<div class="dialog-overlay">
                        <div class="loading-text"></div>
                    </div>{/if}

                <BottomBar {activePanel} ontoggle={togglePanel} />
            </div>
        </Globals>
    </WillowDark>
</div>

<style>
    .side-panel-popup {
        display: flex;
        align-items: stretch;
        flex-shrink: 0;
    }

    /* shared side panel styles */
    :global(.side-panel-popup .panel) {
        position: relative;
        height: 100%;
        box-sizing: border-box;
        padding: 16px;
        display: flex;
        flex-direction: column;
        gap: 12px;
        background: var(--wx-background);
        color: var(--wx-color-font);
    }

    :global(.side-panel-popup .panel-close) {
        display: flex;
        align-items: center;
        justify-content: center;
        width: 22px;
        height: 22px;
        background: none;
        border: none;
        cursor: pointer;
        border-radius: 2px;
        color: var(--tonic-text-dim);
        padding: 0;
        flex-shrink: 0;
        transition:
            color 100ms ease,
            background 100ms ease;
    }
    :global(.side-panel-popup .panel-close:hover) {
        color: var(--wx-color-font);
        background: var(--tonic-btn-hover-bg);
    }
    :global(.side-panel-popup .panel-close svg) {
        width: 10px;
        height: 10px;
    }

    :global(.side-panel-popup .header) {
        display: flex;
        align-items: flex-start;
        justify-content: space-between;
        gap: 14px;
        padding: 0 0 12px;
        border-bottom: var(--wx-border-light);
    }
    :global(.side-panel-popup .header h2) {
        margin: 0;
        font-size: 13px;
        font-family: "JetBrains Mono", monospace;
        color: var(--wx-color-font);
        letter-spacing: 0.03em;
    }
    :global(.side-panel-popup .header p) {
        margin: 3px 0 0;
        opacity: 0.4;
        font-size: 11px;
    }

    :global(.side-panel-popup .actions) {
        display: flex;
        gap: 8px;
        flex-shrink: 0;
    }

    :global(.side-panel-popup button:not(.panel-close)) {
        border: var(--wx-border);
        background: var(--wx-button-background);
        color: var(--wx-color-font);
        border-radius: var(--wx-border-radius);
        padding: 5px 12px;
        font-size: 12px;
        font-family: inherit;
        font-weight: 500;
        cursor: pointer;
        transition:
            background 120ms ease,
            border-color 120ms ease;
    }
    :global(.side-panel-popup button:not(.panel-close):hover) {
        background: var(--tonic-btn-hover-bg);
    }

    :global(.wx-popup) {
        --wx-popup-border: 1px solid var(--tonic-popup-border-color) !important;
        --wx-popup-border-radius: 2px !important;
        --wx-popup-shadow: 0 4px 12px rgba(0, 0, 0, 0.18) !important;
        --wx-popup-background: var(--tonic-popup-bg) !important;
    }

    /* Portals rendered inside .layout-container via data-wx-portal-root.
       Override SVAR's dark vars so light theme portals use correct colors. */
    :global(.layout-container[data-theme="light"] .wx-willow-dark-theme) {
        --wx-color-font: #242529 !important;
        --wx-color-font-alt: #58585a !important;
        --wx-icon-color: #58585a !important;
        --wx-color-primary: #4184bf !important;
        --wx-background: #ffffff !important;
        --wx-background-alt: #f4f5f7 !important;
        --wx-background-hover: #e8e9ec !important;
        --wx-border: 1px solid #c9c9ca !important;
        --wx-border-medium: 1px solid #b8b9bc !important;
        color-scheme: light;
    }

    /* MenuBar: override SVAR default background */
    :global(.wx-menubar) {
        background: transparent !important;
    }

    /* Context menu & dropdown menu */
    :global([data-wx-menu].wx-menu) {
        background: var(--tonic-popup-bg) !important;
        border: 1px solid var(--tonic-popup-border-color) !important;
        border-radius: 2px !important;
        box-shadow: 0 4px 12px rgba(0, 0, 0, 0.18) !important;
    }

    /* Dropdown list background */
    :global(.wx-dropdown .wx-list) {
        --wx-input-padding: 4px 8px;
        --wx-input-font-size: 12px;
        --wx-checkbox-size: 14px;
        --wx-checkbox-height: 14px;
        background: var(--tonic-popup-bg);
    }

    /* Menu options */
    :global([data-wx-menu] .wx-option) {
        font-size: 12px !important;
        height: auto !important;
        min-height: 26px !important;
        line-height: normal !important;
        display: flex !important;
        align-items: center !important;
        border-radius: 4px !important;
        margin: 1px 4px !important;
        padding: 0 8px !important;
        background: transparent !important;
        transition: background 100ms ease;
    }
    :global([data-wx-menu] .wx-option:hover) {
        background: rgba(65, 132, 191, 0.12) !important;
    }
    :global([data-wx-menu] .wx-option.wx-active) {
        background: rgba(65, 132, 191, 0.18) !important;
    }
    :global([data-wx-menu] .wx-option.filter-option),
    :global([data-wx-menu] .wx-option.filter-option:hover) {
        background: transparent !important;
        cursor: default !important;
        padding: 4px 8px !important;
    }
    :global([data-wx-menu] .wx-separator) {
        margin: 4px 8px !important;
        border-color: var(--tonic-row-border-color) !important;
    }

    /* Menu bar icons */
    :global([data-wx-menu] .wx-icon) {
        display: flex !important;
        align-items: center !important;
        justify-content: center !important;
        font-size: 12px !important;
        color: var(--tonic-icon-color) !important;
    }

    /* Filter MultiCombo in dropdown */
    :global(.filter-option .wx-multicombo) {
        font-size: 12px !important;
        --wx-input-height: 26px;
        --wx-input-font-size: 12px;
        --wx-input-padding: 0 6px;
        --wx-input-icon-size: 14px;
        --wx-input-border: var(--wx-border);
        --wx-input-border-focus: 1px solid var(--wx-color-primary);
        --wx-input-background: var(--tonic-popup-bg);
    }
    :global(.filter-option .wx-multicombo .wx-wrapper) {
        min-height: 0 !important;
        border-radius: 2px !important;
    }
    :global(.wx-dropdown) {
        border-radius: 2px !important;
        background: var(--tonic-popup-bg) !important;
        border: 1px solid var(--tonic-popup-border-color) !important;
    }
    :global(.wx-dropdown .wx-list .wx-checkbox) {
        margin-right: 6px !important;
        accent-color: var(--wx-color-primary);
    }
    :global(.wx-dropdown .wx-list .wx-item) {
        border-radius: 4px !important;
        padding: 2px 6px !important;
    }
    :global(.wx-dropdown .wx-list .wx-item.wx-focus) {
        background: transparent !important;
    }

    :global(.wx-dropdown .wx-list .wx-item:hover) {
        background: rgba(65, 132, 191, 0.12) !important;
    }
    .dialog-overlay {
        position: fixed;
        inset: 0;
        z-index: 9999;
        background: rgba(0, 0, 0, 0.35);
        display: flex;
        align-items: center;
        justify-content: center;
        color: rgba(255, 255, 255, 0.8);
        font-size: 14px;
    }

    .loading-text {
        width: fit-content;
        font-size: 44px;
        line-height: 1.5;
        font-family: system-ui, sans-serif;
        font-weight: bold;
        text-transform: uppercase;
        color: #0000;
        -webkit-text-stroke: 1px var(--wx-color-primary);
        background:
            radial-gradient(
                    1.13em at 50% 1.6em,
                    var(--wx-color-primary) 99%,
                    #0000 101%
                )
                calc(50% - 1.6em) 0/3.2em 100% text,
            radial-gradient(
                    1.13em at 50% -0.8em,
                    #0000 99%,
                    var(--wx-color-primary) 101%
                )
                50% 0.8em/3.2em 100% repeat-x text;
        animation: l9 2s linear infinite;
    }
    .loading-text:before {
        content: "Loading";
    }
    @keyframes l9 {
        to {
            background-position:
                calc(50% + 1.6em) 0,
                calc(50% + 3.2em) 0.8em;
        }
    }

    .file-title {
        min-width: 10em;
        display: flex;
        align-items: center;
        justify-content: center;
        gap: 0.5em;
        margin-left: 1em;
        margin-right: 1em;
        font-weight: 500;
        font-family: "JetBrains Mono", monospace;
        font-size: 11px;
        letter-spacing: 0.02em;
        white-space: nowrap;
    }

    .file-status {
        width: 0.85em;
        height: 0.85em;
    }

    .save-icon {
        flex-shrink: 0;
    }

    .spinning {
        animation: spin 1s linear infinite;
    }

    @keyframes spin {
        from {
            transform: rotate(0deg);
        }
        to {
            transform: rotate(360deg);
        }
    }

    :global(html, body) {
        margin: 0;
        padding: 0;
        height: 100%;
        width: 100%;
        overflow: hidden;
    }

    :global(html.busy),
    :global(html.busy *) {
        cursor: wait !important;
    }

    /* scroll bars */
    :global(*) {
        scrollbar-width: thin;
        scrollbar-color: var(--tonic-scrollbar) transparent;
    }
    :global(::-webkit-scrollbar) {
        width: 0.5rem;
        height: 0.5rem;
    }
    :global(::-webkit-scrollbar-track) {
        background: transparent;
    }
    :global(::-webkit-scrollbar-thumb) {
        background: var(--tonic-scrollbar);
        border-radius: 1px;
    }
    :global(::-webkit-scrollbar-thumb:hover) {
        background: var(--tonic-scrollbar-hover);
    }
    :global(::-webkit-scrollbar-corner) {
        background: transparent;
    }

    :global(.noselect) {
        user-select: none !important;
        -webkit-user-select: none !important;
        -moz-user-select: none !important;
        -ms-user-select: none !important;
        cursor: default;
    }

    .root {
        display: flex;
        flex-direction: column;
        height: 100vh;
        width: 100vw;
        min-height: 0;
    }

    .layout-container {
        color-scheme: normal;
        font-size: 12px;
        display: flex;
        flex-direction: column;
        height: 100%;
        width: 100%;
        min-height: 0;
        overflow: hidden;
        position: relative;
        z-index: 0;
        background: var(--wx-background);
        color: var(--wx-color-font);

        /* Font size */
        --wx-font-size: 12px;

        /* Primary: Misso Blue */
        --wx-color-primary: #4184bf;
        --wx-color-primary-selected: rgba(65, 132, 191, 0.25);
        --wx-color-primary-font: #fff;
        --wx-color-secondary: transparent;
        --wx-color-secondary-hover: rgba(65, 132, 191, 0.1);
        --wx-color-secondary-font: var(--wx-color-primary);
        --wx-color-secondary-font-hover: var(--wx-color-primary);
        --wx-color-secondary-border: var(--wx-color-primary);

        /* Semantic */
        --wx-color-success: #4ade80;
        --wx-color-warning: #fbbf24;
        --wx-color-danger: #f87171;
        --wx-color-info: #67b4e0;

        /* Backgrounds: neutral grey with subtle cool tint */
        --wx-background: #2a2c32;
        --wx-background-alt: #32343a;
        --wx-background-hover: #232529;

        /* Borders */
        --wx-border: 1px solid #3c3e44;
        --wx-border-light: 1px solid #35373d;
        --wx-border-medium: 1px solid #44464c;
        --wx-border-radius: 2px;

        /* Table / grid */
        --wx-table-header-background: #232529;
        --wx-table-select-background: #32343a;
        --wx-table-cell-border: 1px solid #35373d;

        /* Buttons */
        --wx-button-background: #32343a;
        --wx-button-pressed: #2a2c32;
        --wx-button-primary-pressed: #2d6a9e;

        /* Input */
        --wx-input-background: var(--wx-background);
        --wx-input-background-disabled: #32343a;
        --wx-input-border: var(--wx-border);
        --wx-input-border-focus: 1px solid var(--wx-color-primary);

        /* Tabs */
        --wx-tabs-active-color: var(--wx-color-primary);
        --wx-tabs-active-border: var(--wx-color-primary);

        /* Switch/slider */
        --wx-switch-background: #3c3e44;
        --wx-slider-background: #32343a;

        /* Disabled */
        --wx-color-disabled: #44464c;
        --wx-color-disabled-alt: #52545a;

        /* Popups above grid overlays */
        --wx-popup-z-index: 10;

        /* Notice */
        --wx-notice-background: #32343a;
        --wx-notice-border: 1px solid #3c3e44;
        --wx-notice-border-radius: 2px;
        --wx-notice-type-icon-color: #a0a0a0;

        /* tonic tokens: dark */
        --tonic-scrollbar: #3c3e44;
        --tonic-scrollbar-hover: #52545a;
        --tonic-popup-bg: #1e2024;
        --tonic-popup-border-color: #44464c;
        --tonic-popup-font: rgba(255, 255, 255, 0.9);
        --tonic-icon-color: #c0c0c0;
        --tonic-btn-hover-bg: #3c3e44;
        --tonic-row-num-color: rgba(255, 255, 255, 0.55);
        --tonic-col-header-color: rgba(255, 255, 255, 0.6);
        --tonic-highlight-bg: #1e2024;
        --tonic-table-header-cell-bg: #1a1c22;
        --tonic-table-header-cell-border-color: #44464c;
        --tonic-table-header-cell-color: #c0c0c0;
        --tonic-row-even-bg: #1e2026;
        --tonic-row-odd-bg: #22242a;
        --tonic-row-border-color: #35373d;
        --tonic-window-btn-color: rgba(255, 255, 255, 0.5);
        --tonic-text-dim: rgba(255, 255, 255, 0.5);
        --tonic-text-muted: rgba(255, 255, 255, 0.25);
        --tonic-cell-tint: rgba(255, 255, 255, 0.07);
        --tonic-cell-tint-strong: rgba(255, 255, 255, 0.13);
    }

    .layout-container[data-theme="light"] {
        color-scheme: light;
        font-weight: 500;

        /* svar overrides: Zed One Light palette */
        --wx-font-weight: 500;
        --wx-color-font: #242529;
        --wx-color-font-alt: #58585a;
        --wx-color-font-disabled: #9a9ba0;
        /* #ebebec = Zed panel/sidebar, app chrome bg */
        --wx-background: #ebebec;
        /* #fafafa = Zed editor/content, near-white, but not pure white */
        --wx-background-alt: #fafafa;
        --wx-background-hover: #dfdfe0;
        --wx-border: 1px solid #c9c9ca;
        --wx-border-light: 1px solid #dfdfe0;
        --wx-border-medium: 1px solid #b8b9bc;
        /* #e2e3e5 = clearly grey gutter for row nums / col headers */
        --wx-table-header-background: #dddfe2;
        --wx-table-select-background: #d0d8e8;
        --wx-table-cell-border: 1px solid #c4c5c8;
        /* Direct values: avoids variable chain resolution issues on WebKitGTK */
        --wx-table-header-border: 1px solid #c9c9ca;
        --wx-table-header-cell-border: 1px solid #c9c9ca;
        --wx-button-background: #e5e6e8;
        --wx-button-pressed: #d8d9db;
        --wx-button-primary-pressed: #2d6a9e;
        --wx-input-background: #fafafa;
        --wx-input-background-disabled: #e5e6e8;
        --wx-input-border: 1px solid #c9c9ca;
        --wx-switch-background: #b0b1b4;
        --wx-slider-background: #e5e6e8;
        --wx-color-disabled: #c9c9ca;
        --wx-color-disabled-alt: #b0b1b4;
        --wx-notice-background: #fafafa;
        --wx-notice-border: 1px solid #c9c9ca;
        --wx-notice-type-icon-color: #6e6f74;

        /* tonic tokens: light */
        --tonic-content-bg: #fafafa;
        --tonic-scrollbar: #b8b9bc;
        --tonic-scrollbar-hover: #9a9b9e;
        --tonic-popup-bg: #ffffff;
        --tonic-popup-border-color: #c9c9ca;
        --tonic-popup-font: #242529;
        --tonic-icon-color: #58585a;
        --tonic-btn-hover-bg: #dfdfe0;
        --tonic-row-num-color: #58585a;
        --tonic-col-header-color: #58585a;
        --tonic-highlight-bg: #d5e5f5;
        --tonic-table-header-cell-bg: #d4d5da;
        --tonic-table-header-cell-border-color: #b8b9bc;
        --tonic-table-header-cell-color: #242529;
        --tonic-row-even-bg: #f5f5f7;
        --tonic-row-odd-bg: #eaeaed;
        --tonic-row-border-color: #d8d9dc;
        --tonic-window-btn-color: #6e6f74;
        --tonic-text-dim: #66676a;
        --tonic-text-muted: #8e8f92;
        --tonic-cell-tint: rgba(0, 0, 0, 0.035);
        --tonic-cell-tint-strong: rgba(0, 0, 0, 0.07);
    }
</style>
