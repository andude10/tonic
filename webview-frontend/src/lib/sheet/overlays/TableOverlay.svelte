<script lang="ts">
    import {
        cellRangeToPixels,
        type CellRange,
        type SheetObjectsState,
        type PixelRect,
    } from "./Overlays.svelte";

    let {
        sos,
        headerBounds,
        bodyBounds,
        title = "Table",
        hasShadow = false,
    }: {
        sos: SheetObjectsState;
        headerBounds: CellRange;
        bodyBounds: CellRange;
        title?: string;
        hasShadow?: boolean;
    } = $props();

    let borderRect: PixelRect | null = $state(null);
    let titleRect: PixelRect | null = $state(null);

    const TITLE_HEIGHT = 18;

    let fullBounds: CellRange = $derived({
        minR: headerBounds.minR,
        maxR: bodyBounds.maxR,
        minC: Math.min(headerBounds.minC, bodyBounds.minC),
        maxC: Math.max(headerBounds.maxC, bodyBounds.maxC),
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

{#if borderRect}
    <div
        class="table-border"
        class:has-shadow={hasShadow}
        style:left="{borderRect.left}px"
        style:top="{borderRect.top}px"
        style:width="{borderRect.width}px"
        style:height="{borderRect.height}px"
    ></div>
{/if}
{#if titleRect}
    <div
        class="table-title"
        style:left="{titleRect.left}px"
        style:top="{titleRect.top}px"
        style:width="{titleRect.width}px"
        style:height="{titleRect.height}px"
    >
        <span class="title-text">{title}</span>
    </div>
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

    .table-title {
        position: absolute;
        color: #a1a1aa;
        font-size: 12px;
        font-weight: 600;
        display: flex;
        align-items: center;
        justify-content: center;
        pointer-events: none;
        overflow: hidden;
        white-space: nowrap;
        background: #1a1a1a;
    }

    .title-text {
        text-overflow: ellipsis;
        overflow: hidden;
    }

    .table-border.has-shadow {
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
