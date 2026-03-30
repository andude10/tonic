<script lang="ts">
    import {
        cellRangeToPixels,
        toTranslate3d,
        type CellRange,
        type SheetObjectsState,
        type PixelRect,
    } from "./Overlays.svelte";

    let {
        sos,
        bounds,
        visible,
        isFilling,
        isEditing = false,
        editorInputWidth = 0,
        showBorder = true,
        showBackground = false,
        zIndex = 2,
        onfillstart,
    }: {
        sos: SheetObjectsState;
        bounds: CellRange | null;
        visible: boolean;
        isFilling: boolean;
        isEditing?: boolean;
        editorInputWidth?: number;
        showBorder?: boolean;
        showBackground?: boolean;
        zIndex?: number;
        onfillstart: (ev: MouseEvent) => void;
    } = $props();

    let rect: PixelRect | null = $state(null);

    export function reposition() {
        rect = bounds ? cellRangeToPixels(sos, bounds) : null;
    }

    let show = $derived(visible && !!rect);
</script>

{#if show}
    <div
        class="focus-overlay"
        style:transform={toTranslate3d(rect!)}
        style:width="{Math.max(
            rect!.width,
            isEditing ? editorInputWidth : 0,
        )}px"
        style:height="{rect!.height}px"
        style:border-color={showBorder
            ? "var(--wx-color-primary)"
            : "transparent"}
        style:background={showBackground
            ? "color-mix(in srgb, var(--wx-color-primary), transparent 88%)"
            : "transparent"}
        style:z-index={zIndex}
    >
        {#if showBorder}
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div
                class="fill-handle"
                class:filling={isFilling}
                onmousedown={onfillstart}
            ></div>
        {/if}
    </div>
{/if}

<style>
    .focus-overlay {
        position: absolute;
        border: 2px solid transparent;
        border-radius: 4px;
        background: transparent;
        box-sizing: border-box;
        will-change: transform, width, height;
        pointer-events: none;
        --transition-base:
            transform 30ms cubic-bezier(0, 0, 0.2, 1),
            width 30ms cubic-bezier(0, 0, 0.2, 1),
            height 30ms cubic-bezier(0, 0, 0.2, 1);
        transition: var(--transition-base);
    }

    .fill-handle {
        position: absolute;
        bottom: -4px;
        right: -4px;
        width: 9px;
        height: 9px;
        background: var(--wx-color-primary);
        border: 1px solid var(--wx-background);
        cursor: crosshair;
        pointer-events: auto;
        z-index: 10;
    }

    .fill-handle.filling {
        width: 12px;
        height: 12px;
        bottom: -6px;
        right: -6px;
        animation: fill-spin 0.8s linear infinite;
    }

    @keyframes fill-spin {
        from {
            transform: rotate(0deg);
        }
        to {
            transform: rotate(360deg);
        }
    }
</style>
