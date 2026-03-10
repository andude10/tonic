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
    import { onMount } from "svelte";

    attachConsole();
    trackFps();

    let sheet: Sheet;
    let currentFilePath: string | null = $state(null);
    let fileTitle: string | null = $state(null);
    let dialogOpen = $state(false);

    const DIALOG_FILTER = { name: "Tonic Spreadsheet", extensions: ["tcs"] };

    async function syncFileInfo() {
        const [name, path] =
            await invoke<[string | null, string | null]>("get_file_info");
        currentFilePath = path;
        fileTitle = name;
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
                await invoke("open_file", { path });
                await syncFileInfo();
                sheet.onFileLoad();
                break;
            }
            case "file-save":
                if (currentFilePath) {
                    await invoke("save_file", { path: currentFilePath });
                } else {
                    dialogOpen = true;
                    const path = await save({ filters: [DIALOG_FILTER] });
                    dialogOpen = false;
                    if (!path) return;
                    await invoke("save_file", { path });
                }
                await syncFileInfo();
                break;
            case "file-save-as": {
                dialogOpen = true;
                const path = await save({ filters: [DIALOG_FILTER] });
                dialogOpen = false;
                if (!path) return;
                await invoke("save_file", { path });
                await syncFileInfo();
                break;
            }
        }
    }

    onMount(async () => {
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

<div class="root noselect">
    <WillowDark>
        <div class="layout-container">
            <WindowBar>
                <span class="file-title">{fileTitle}</span>
                <MenuBar options={menu_options} onclick={onMenuClick}></MenuBar>
            </WindowBar>

            <Sheet bind:this={sheet} />
            {#if dialogOpen}<div class="dialog-overlay"></div>{/if}

            <DevBottomPanel />
        </div>
    </WillowDark>
</div>

<style>
    /* Menu bar options */
    :global([data-wx-menu] .wx-option) {
        font-size: 12px !important;
        height: 28px !important;
        display: flex !important;
        align-items: center !important;
    }

    /* Menu bar icons */
    :global([data-wx-menu] .wx-icon) {
        display: flex !important;
        align-items: center !important;
        justify-content: center !important;
        font-size: 14px !important;
    }

    .dialog-overlay {
        position: fixed;
        inset: 0;
        z-index: 9999;
        background: rgba(0, 0, 0, 0.3);
    }

    .file-title {
        min-width: 7em;
        display: flex;
        align-items: center;
        justify-content: center;
        margin-left: 1em;
        margin-right: 1em;
        font-weight: 500;
        color: rgba(255, 255, 255, 0.7);
        white-space: nowrap;
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
        --wx-color-success: #9ad636;
        --wx-color-warning: #ffd24d;
        --wx-color-danger: #ff6b70;
        --wx-color-info: #2a96d6;

        /* Dark backgrounds — neutral grey with slight cool tint */
        --wx-background: #2b2d30;
        --wx-background-alt: #363839;
        --wx-background-hover: #222426;

        /* Borders */
        --wx-border: 1px solid #3e4042;
        --wx-border-light: 1px solid #3e4042;
        --wx-border-medium: 1px solid #3e4042;

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
