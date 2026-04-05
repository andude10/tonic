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
        style:transform={toTranslate3d(rect!)}
        style:width="{rect!.width}px"
        style:height="{rect!.height}px"
    ></div>
{/if}

<style>
    .selection-overlay {
        position: absolute;
        border: 1px solid rgba(65, 132, 191, 0.5);
        border-radius: 1px;
        background: rgba(65, 132, 191, 0.06);
        will-change: transform, width, height;
        pointer-events: none;
        transition:
            transform 30ms cubic-bezier(0, 0, 0.2, 1),
            width 30ms cubic-bezier(0, 0, 0.2, 1),
            height 30ms cubic-bezier(0, 0, 0.2, 1);
    }
</style>
