<script lang="ts">
    import {
        cellRangeToPixels,
        type CellRange,
        type SheetObjectsState,
        type PixelRect,
    } from "./Overlays.svelte";

    let {
        sos,
        bounds,
        visible = true,
    }: {
        sos: SheetObjectsState;
        bounds: CellRange | null;
        visible?: boolean;
    } = $props();

    let rect: PixelRect | null = $state(null);

    export function reposition() {
        rect = bounds ? cellRangeToPixels(sos, bounds) : null;
    }

    let show = $derived(visible && !!rect);
</script>

{#if show}
    <div
        class="selection-overlay"
        style:left="{rect!.left}px"
        style:top="{rect!.top}px"
        style:width="{rect!.width}px"
        style:height="{rect!.height}px"
    ></div>
{/if}

<style>
    .selection-overlay {
        position: absolute;
        border: 1px solid color-mix(in srgb, var(--wx-color-primary), white 18%);
        border-radius: 3px;
        background: color-mix(
            in srgb,
            var(--wx-color-primary),
            transparent 95%
        );
        will-change: left, top, width, height;
        pointer-events: none;
        transition:
            left 30ms cubic-bezier(0, 0, 0.2, 1),
            top 30ms cubic-bezier(0, 0, 0.2, 1),
            width 30ms cubic-bezier(0, 0, 0.2, 1),
            height 30ms cubic-bezier(0, 0, 0.2, 1);
    }
</style>
