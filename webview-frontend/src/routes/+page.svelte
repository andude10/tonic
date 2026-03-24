<script lang="ts">
    import { WillowDark } from "@svar-ui/svelte-grid";
    import { MenuBar } from "@svar-ui/svelte-menu";
    import { menu_options } from "$lib/data";
    import WindowBar from "$lib/WindowBar.svelte";
    import Sheet from "$lib/sheet/Sheet.svelte";
    import DevBottomPanel from "$lib/DevBottomPanel.svelte";
    import { trackFps } from "$lib/stats.svelte";
    import type { IApi } from "@svar-ui/svelte-grid";
    import { attachConsole } from "@tauri-apps/plugin-log";
    import { getCurrentWindow } from "@tauri-apps/api/window";
    import { invoke } from "@tauri-apps/api/core";
    import { open, save } from "@tauri-apps/plugin-dialog";
    import { listen } from "@tauri-apps/api/event";
    import { onMount } from "svelte";

    attachConsole();
    trackFps();

    let sheet: Sheet;
    let currentFilePath: string | null = $state(null);
    let fileTitle: string | null = $state(null);
    let dialogOpen = $state(false);
    let isSaved = $state(true);
    let isSaving = $state(false);
    let isLoading = $state(false);

    const DIALOG_FILTER = { name: "Tonic Spreadsheet", extensions: ["tcs"] };

    async function syncFileInfo() {
        const [name, path] =
            await invoke<[string | null, string | null]>("get_file_info");
        currentFilePath = path;
        fileTitle = name;
    }

    async function commitSaveAs() {
        dialogOpen = true;
        const path = await save({ filters: [DIALOG_FILTER] });
        dialogOpen = false;
        if (!path) return;
        isSaving = true;
        const uiDecorationsJson = sheet.saveDecorationsToJson();
        await invoke("save_file", { path, uiDecorationsJson });
        isSaving = false;
        await syncFileInfo();
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
            await syncFileInfo();
        } else {
            await commitSaveAs();
        }
    }

    async function onMenuClick(ev: any) {
        const id = ev.action?.id;
        if (!id) return;
        switch (id) {
            case "file-new":
                await invoke("new_file");
                await syncFileInfo();
                sheet.onFileLoad();
                break;
            case "file-open": {
                dialogOpen = true;
                const path = await open({
                    multiple: false,
                    directory: false,
                    filters: [DIALOG_FILTER],
                });
                dialogOpen = false;
                if (!path) return;
                isLoading = true;
                const decorationsJson = await invoke<string>("open_file", {
                    path,
                });
                isLoading = false;
                await syncFileInfo();
                sheet.onFileLoad(decorationsJson);
                break;
            }
            case "file-save":
                await commitSave();
                break;
            case "file-save-as":
                await commitSaveAs();
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
            await syncFileInfo();
            el.textContent = fileTitle ?? "";
        } catch (e) {
            console.error("Rename failed:", e);
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

        // create new file if none is open
        await syncFileInfo();
        if (!currentFilePath) {
            await invoke("new_file");
        }
        await syncFileInfo();

        // on start-up, window flashes white screen before rendering
        // it is known webview issue: https://github.com/tauri-apps/tauri/issues/1564
        // this allows to show window once everything is loaded
        getCurrentWindow().show();
    });
</script>

<div class="root noselect" onkeydowncapture={handleKeyDown}>
    <WillowDark>
        <div class="layout-container">
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
                                fill="#aaa"
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
                                fill="#699f4c"
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
                                fill="#9f4c4c"
                                fill-rule="evenodd"
                                d="M940,510a30,30,0,1,1,30-30A30,30,0,0,1,940,510Zm15-20.047A3.408,3.408,0,0,1,955,494.77l-0.221.22a3.42,3.42,0,0,1-4.833,0l-8.764-8.755a1.71,1.71,0,0,0-2.417,0l-8.741,8.747a3.419,3.419,0,0,1-4.836,0l-0.194-.193a3.408,3.408,0,0,1,.017-4.842l8.834-8.735a1.7,1.7,0,0,0,0-2.43l-8.831-8.725a3.409,3.409,0,0,1-.018-4.844l0.193-.193a3.413,3.413,0,0,1,2.418-1c0.944,0,3.255,1.835,3.872,2.455l7.286,7.287a1.708,1.708,0,0,0,2.417,0l8.764-8.748a3.419,3.419,0,0,1,4.832,0L955,465.243a3.408,3.408,0,0,1,0,4.818l-8.727,8.737a1.7,1.7,0,0,0,0,2.407Z"
                                transform="translate(-910 -450)"
                            />
                        </svg>
                    {/if}
                </span>
                <MenuBar options={menu_options} onclick={onMenuClick}></MenuBar>
            </WindowBar>

            <Sheet bind:this={sheet} />
            {#if dialogOpen}<div class="dialog-overlay"></div>{/if}
            {#if isLoading}<div class="dialog-overlay">
                    <div class="loading-text"></div>
                </div>{/if}

            <DevBottomPanel />
        </div>
    </WillowDark>
</div>

<style>
    :global(.wx-popup) {
        --wx-popup-border: 1px solid rgba(255, 255, 255, 0.08) !important;
        --wx-popup-border-radius: 8px !important;
        --wx-popup-shadow:
            0 8px 24px rgba(0, 0, 0, 0.35), 0 2px 6px rgba(0, 0, 0, 0.2) !important;
        --wx-popup-background: #1e1f22 !important;
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
        transition: background 100ms ease;
    }
    :global([data-wx-menu] .wx-option:hover) {
        background: rgba(126, 93, 171, 0.15) !important;
    }
    :global([data-wx-menu] .wx-option.wx-active) {
        background: rgba(126, 93, 171, 0.22) !important;
    }
    :global([data-wx-menu] .wx-option.filter-option),
    :global([data-wx-menu] .wx-option.filter-option:hover) {
        background: transparent !important;
        cursor: default !important;
        padding: 4px 8px !important;
    }
    :global([data-wx-menu] .wx-separator) {
        margin: 4px 8px !important;
        border-color: rgba(255, 255, 255, 0.06) !important;
    }

    /* Menu bar icons */
    :global([data-wx-menu] .wx-icon) {
        display: flex !important;
        align-items: center !important;
        justify-content: center !important;
        font-size: 12px !important;
    }

    /* Filter MultiCombo in dropdown */
    :global(.filter-option .wx-multicombo) {
        font-size: 12px !important;
        --wx-input-height: 26px;
        --wx-input-font-size: 12px;
        --wx-input-padding: 0 6px;
        --wx-input-icon-size: 14px;
        --wx-input-border: 1px solid rgba(255, 255, 255, 0.08);
        --wx-input-border-focus: 1px solid var(--wx-color-primary);
        --wx-input-background: #262729;
    }
    :global(.filter-option .wx-multicombo .wx-wrapper) {
        min-height: 0 !important;
        border-radius: 6px !important;
    }
    :global(.wx-dropdown .wx-list) {
        --wx-input-padding: 4px 8px;
        --wx-input-font-size: 12px;
        --wx-checkbox-size: 14px;
        --wx-checkbox-height: 14px;
    }
    :global(.wx-dropdown) {
        border-radius: 6px !important;
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
        background: rgba(126, 93, 171, 0.12) !important;
    }
    .dialog-overlay {
        position: fixed;
        inset: 0;
        z-index: 9999;
        background: rgba(0, 0, 0, 0.3);
        display: flex;
        align-items: center;
        justify-content: center;
        color: rgba(255, 255, 255, 0.8);
        font-size: 14px;
    }

    .loading-text {
        width: fit-content;
        font-size: 40px;
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
        color: rgba(255, 255, 255, 0.7);
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

    /* scroll bars */
    :global(*) {
        scrollbar-width: thin;
        scrollbar-color: #3e4042 transparent;
    }
    :global(::-webkit-scrollbar) {
        width: 0.5rem;
        height: 0.5rem;
    }
    :global(::-webkit-scrollbar-track) {
        background: transparent;
    }
    :global(::-webkit-scrollbar-thumb) {
        background: #3e4042;
        border-radius: 0.25rem;
    }
    :global(::-webkit-scrollbar-thumb:hover) {
        background: var(--wx-color-primary, #7e5dab);
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

        /* Font size */
        --wx-font-size: 12px;

        /* Vibrant Summer color palette overrides */

        /* Primary: Dusty Grape */
        --wx-color-primary: #7e5dab;
        --wx-color-primary-selected: rgba(126, 93, 171, 0.3);
        --wx-color-primary-font: #fff;
        --wx-color-secondary: transparent;
        --wx-color-secondary-hover: rgba(126, 93, 171, 0.12);
        --wx-color-secondary-font: var(--wx-color-primary);
        --wx-color-secondary-font-hover: var(--wx-color-primary);
        --wx-color-secondary-border: var(--wx-color-primary);

        /* Semantic colors from Vibrant Summer palette */
        --wx-color-success: #22c55e;
        --wx-color-warning: #eab308;
        --wx-color-danger: #ef4444;
        --wx-color-info: #3b82f6;

        /* Secondary font color for icons */
        --wx-color-font-alt: white;

        /* Dark backgrounds — neutral grey with slight cool tint */
        --wx-background: #2b2d30;
        --wx-background-alt: #363839;
        --wx-background-hover: #222426;

        /* Borders */
        --wx-border: 1px solid #3e4042;
        --wx-border-light: 1px solid #3e4042;
        --wx-border-medium: 1px solid #3e4042;
        --wx-border-radius: 6px;

        /* Table overrides */
        --wx-table-header-background: #222426;
        --wx-table-select-background: #363839;

        /* Buttons */
        --wx-button-background: #363839;
        --wx-button-pressed: #2e3032;
        --wx-button-primary-pressed: #4a3566;

        /* Input */
        --wx-input-background: var(--wx-background);
        --wx-input-background-disabled: #363839;
        --wx-input-border: var(--wx-border);
        --wx-input-border-focus: 1px solid var(--wx-color-primary);

        /* Tabs */
        --wx-tabs-active-color: var(--wx-color-primary);
        --wx-tabs-active-border: var(--wx-color-primary);

        /* Switch/slider */
        --wx-switch-background: #3e4042;
        --wx-slider-background: #363839;

        /* Color disabled */
        --wx-color-disabled: #3e4042;
        --wx-color-disabled-alt: #464849;

        background: var(--wx-background);
    }
</style>
