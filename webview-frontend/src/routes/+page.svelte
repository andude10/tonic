<script lang="ts">
    import { WillowDark } from "@svar-ui/svelte-grid";
    import { MenuBar } from "@svar-ui/svelte-menu";
    import { menu_options } from "$lib/data";
    import WindowBar from "$lib/WindowBar.svelte";
    import Sheet from "$lib/Sheet.svelte";
    import DevBottomPanel from "$lib/DevBottomPanel.svelte";
    import { timeRenders } from "$lib/devBottomPanelApi.svelte";
    import type { IApi } from "@svar-ui/svelte-grid";
    import { attachConsole } from "@tauri-apps/plugin-log";

    attachConsole();
    timeRenders();
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
    }

    .layout-container {
        display: flex;
        flex-direction: column;
        height: 100%;
        width: 100%;
        overflow: hidden;
        position: relative;
    }
</style>
