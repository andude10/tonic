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

{#if show}
    <div
        class="fill-origin-overlay"
        style:transform={toTranslate3d(rect!)}
        style:width="{rect!.width}px"
        style:height="{rect!.height}px"
    ></div>
{/if}

<style>
    .fill-origin-overlay {
        position: absolute;
        background: rgba(65, 132, 191, 0.06);
        border: 1px dashed rgba(65, 132, 191, 0.35);
        border-radius: 1px;
        pointer-events: none;
    }
</style>
