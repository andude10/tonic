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
        background: color-mix(in srgb, var(--wx-color-primary) 8%, transparent);
        border: 2px dashed
            color-mix(in srgb, var(--wx-color-primary) 40%, transparent);
        border-radius: 2px;
        pointer-events: none;
    }
</style>
