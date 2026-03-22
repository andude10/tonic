<script lang="ts">
    import { invoke } from "@tauri-apps/api/core";
    import {
        cellRangeToPixels,
        type CellRange,
        type SheetObjectsState,
        type PixelRect,
    } from "./Overlays.svelte";
    import type { TableData } from "$lib/sheet/shared";

    let {
        sos,
        table,
    }: {
        sos: SheetObjectsState;
        table: TableData;
    } = $props();

    function handleTitleBlur(e: Event) {
        const el = e.target as HTMLElement;
        const newName = (el.textContent ?? "").trim();
        if (!newName || newName === table.title) {
            el.textContent = table.title;
            return;
        }
        const oldName = table.title;
        invoke("change_table_name", { oldName, newName })
            .then(() => {
                table.title = newName;
            })
            .catch((err) => {
                console.error("Rename table failed:", err);
                el.textContent = table.title;
            });
    }

    function handleTitleKeyDown(e: KeyboardEvent) {
        if (e.key === "Enter") {
            e.preventDefault();
            (e.target as HTMLElement).blur();
        }
    }

    let borderRect: PixelRect | null = $state(null);
    let titleRect: PixelRect | null = $state(null);

    const TITLE_HEIGHT = 18;

    let fullBounds: CellRange = $derived({
        minR: table.headerBounds.minR,
        maxR: table.bodyBounds.maxR,
        minC: Math.min(table.headerBounds.minC, table.bodyBounds.minC),
        maxC: Math.max(table.headerBounds.maxC, table.bodyBounds.maxC),
    });

    export function reposition() {
        const fr = cellRangeToPixels(sos, fullBounds);
        if (fr) {
            borderRect = {
                left: fr.left,
                top: fr.top - TITLE_HEIGHT,
                width: fr.width,
                height: fr.height + TITLE_HEIGHT,
            };
            titleRect = {
                left: fr.left,
                top: fr.top - TITLE_HEIGHT,
                width: fr.width,
                height: TITLE_HEIGHT + 2,
            };
        } else {
            borderRect = null;
            titleRect = null;
        }
    }
</script>

{#if titleRect}
    <div
        class="table-title"
        style:left="{titleRect.left}px"
        style:top="{titleRect.top}px"
        style:width="{titleRect.width}px"
        style:height="{titleRect.height}px"
    >
        <span
            class="title-text"
            contenteditable="true"
            role="textbox"
            tabindex="0"
            style="outline: none"
            onblur={handleTitleBlur}
            onkeydown={handleTitleKeyDown}>{table.title}</span
        >
    </div>
{/if}
{#if borderRect}
    <div
        class="table-border"
        class:has-projection={table.hasProjection}
        style:left="{borderRect.left}px"
        style:top="{borderRect.top}px"
        style:width="{borderRect.width}px"
        style:height="{borderRect.height}px"
    ></div>
{/if}

<style>
    .table-border {
        position: absolute;
        border-radius: 4px;
        pointer-events: none;
        border: 1px solid #1a1a1a;
        outline: 1px solid #1a1a1a;
        transition: box-shadow 0.3s ease;
        box-shadow:
            0 0 0 0 transparent,
            0 0 0 transparent,
            0 0 0 transparent,
            0 0 0 transparent,
            0 0 0 transparent;
    }

    .table-border.has-projection::after {
        content: "";
        position: absolute;
        inset: -2px;
        border-radius: 4px;
        background:
            linear-gradient(90deg, var(--wx-color-primary) 50%, transparent 0) 0
                0 / 20px 1px repeat-x,
            linear-gradient(90deg, var(--wx-color-primary) 50%, transparent 0) 0
                100% / 20px 1px repeat-x,
            linear-gradient(0deg, var(--wx-color-primary) 50%, transparent 0) 0
                0 / 1px 20px repeat-y,
            linear-gradient(0deg, var(--wx-color-primary) 50%, transparent 0)
                100% 0 / 1px 20px repeat-y;
        animation: table-ants 0.8s linear infinite;
    }

    @keyframes table-ants {
        100% {
            background-position:
                20px 0,
                -20px 100%,
                0 -20px,
                100% 20px;
        }
    }

    .table-title {
        position: absolute;
        color: #a1a1aa;
        font-size: 12px;
        font-weight: 600;
        display: flex;
        align-items: center;
        justify-content: center;
        pointer-events: auto;
        overflow: hidden;
        white-space: nowrap;
        background: #1a1a1a;
    }

    .title-text {
        text-overflow: ellipsis;
        overflow: hidden;
    }

    .table-border.has-projection {
        box-shadow:
        /* tight outline */
            0 0 0 1px rgba(255, 255, 255, 0.06),
            /* inner glow for metallic edge */ inset 0 1px 0
                rgba(255, 255, 255, 0.08),
            inset 0 -1px 0 rgba(0, 0, 0, 0.3),
            /* symmetric ambient shadow */ 0 0 12px rgba(0, 0, 0, 0.5),
            0 0 30px rgba(0, 0, 0, 0.3);
    }
</style>
