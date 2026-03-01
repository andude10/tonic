<script lang="ts">
    import { WillowDark } from "@svar-ui/svelte-grid";
    import { MenuBar } from "@svar-ui/svelte-menu";
    import { menu_options } from "$lib/data";
    import WindowBar from "$lib/WindowBar.svelte";
    import Sheet from "$lib/sheet/Sheet.svelte";
    import DevBottomPanel from "$lib/DevBottomPanel.svelte";
    import { timeRenders } from "$lib/stats.svelte";
    import type { IApi } from "@svar-ui/svelte-grid";
    import { attachConsole } from "@tauri-apps/plugin-log";
    import { getCurrentWindow } from "@tauri-apps/api/window";
    import { onMount } from "svelte";

    attachConsole();
    timeRenders();

    // on start-up, window flashes white screen before rendering
    // it is known webview issue: https://github.com/tauri-apps/tauri/issues/1564
    // this allows to show window once everything is loaded
    onMount(() => {
        getCurrentWindow().show();
    });
</script>

<div class="root noselect">
    <WillowDark>
        <div class="layout-container">
            <WindowBar>
                <MenuBar options={menu_options}></MenuBar>
            </WindowBar>

            <Sheet />

            <DevBottomPanel />
        </div>
    </WillowDark>
</div>

<style>
    :global(html, body) {
        margin: 0;
        padding: 0;
        height: 100%;
        width: 100%;
        overflow: hidden;
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
        display: flex;
        flex-direction: column;
        height: 100%;
        width: 100%;
        min-height: 0;
        overflow: hidden;
        position: relative;

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
