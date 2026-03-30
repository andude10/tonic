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
    }: {
        sos: SheetObjectsState;
        bounds: CellRange | null;
        visible: boolean;
    } = $props();

    let rect: PixelRect | null = $state(null);

    export function reposition() {
        rect = bounds ? cellRangeToPixels(sos, bounds) : null;
    }

    let show = $derived(visible && !!rect);
</script>

{#if show && rect}
    <div
        class="clone-source-overlay"
        style:transform={toTranslate3d(rect)}
        style:width="{rect.width}px"
        style:height="{rect.height}px"
    ></div>
{/if}

<style>
    .clone-source-overlay {
        position: absolute;
        border: 2px dashed var(--wx-color-primary);
        border-radius: 4px;
        will-change: transform, width, height;
        pointer-events: none;
    }
</style>
