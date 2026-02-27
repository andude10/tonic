<script lang="ts">
    import type { Snippet } from "svelte";
    import { getCurrentWindow } from "@tauri-apps/api/window";

    let { children }: { children: Snippet } = $props();

    const win = getCurrentWindow();
</script>

<div class="titlebar" data-tauri-drag-region>
    {@render children()}
    <div class="window-controls">
        <button onclick={() => win.minimize()} aria-label="Minimize">
            <svg
                xmlns="http://www.w3.org/2000/svg"
                width="10"
                height="1"
                viewBox="0 0 10 1"
            >
                <line
                    x1="0"
                    y1="0.5"
                    x2="10"
                    y2="0.5"
                    stroke="currentColor"
                    stroke-width="1"
                />
            </svg>
        </button>
        <button onclick={() => win.toggleMaximize()} aria-label="Maximize">
            <svg
                xmlns="http://www.w3.org/2000/svg"
                width="10"
                height="10"
                viewBox="0 0 10 10"
            >
                <rect
                    x="0.5"
                    y="0.5"
                    width="9"
                    height="9"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="1"
                />
            </svg>
        </button>
        <button class="close" onclick={() => win.close()} aria-label="Close">
            <svg
                xmlns="http://www.w3.org/2000/svg"
                width="10"
                height="10"
                viewBox="0 0 10 10"
            >
                <line
                    x1="0"
                    y1="0"
                    x2="10"
                    y2="10"
                    stroke="currentColor"
                    stroke-width="1"
                />
                <line
                    x1="10"
                    y1="0"
                    x2="0"
                    y2="10"
                    stroke="currentColor"
                    stroke-width="1"
                />
            </svg>
        </button>
    </div>
</div>

<style>
    .titlebar {
        display: flex;
        align-items: center;
        justify-content: space-between;
        width: 100%;
    }

    .window-controls {
        position: fixed;
        top: 0;
        right: 0;
        display: flex;
    }

    .window-controls button {
        background: none;
        border: none;
        color: inherit;
        width: 46px;
        height: 32px;
        cursor: pointer;
        display: flex;
        align-items: center;
        justify-content: center;
    }

    .window-controls button:hover {
        background: rgba(255, 255, 255, 0.1);
    }

    .window-controls .close:hover {
        background: #ff6b70;
    }
</style>
