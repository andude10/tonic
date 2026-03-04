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
        style:left="{rect.left}px"
        style:top="{rect.top}px"
        style:width="{rect.width}px"
        style:height="{rect.height}px"
    ></div>
{/if}

<style>
    .clone-source-overlay {
        position: absolute;
        border: 2px solid var(--wx-color-primary);
        border-radius: 4px;
        will-change: left, top, width, height;
        pointer-events: none;
        --transition-base:
            left 30ms cubic-bezier(0, 0, 0.2, 1),
            top 30ms cubic-bezier(0, 0, 0.2, 1),
            width 30ms cubic-bezier(0, 0, 0.2, 1),
            height 30ms cubic-bezier(0, 0, 0.2, 1);
        transition: var(--transition-base);
        box-shadow:
            0 0 3px 1px
                color-mix(in srgb, var(--wx-color-primary) 40%, transparent),
            0 0 8px 2px
                color-mix(in srgb, var(--wx-color-primary) 20%, transparent);
        animation: clone-glow 1.8s ease-in-out infinite alternate;
    }

    @keyframes clone-glow {
        0% {
            box-shadow:
                0 0 3px 1px
                    color-mix(in srgb, var(--wx-color-primary) 40%, transparent),
                0 0 8px 2px
                    color-mix(in srgb, var(--wx-color-primary) 20%, transparent);
        }
        100% {
            box-shadow:
                0 0 5px 2px
                    color-mix(in srgb, var(--wx-color-primary) 50%, transparent),
                0 0 12px 4px
                    color-mix(in srgb, var(--wx-color-primary) 30%, transparent);
        }
    }
</style>
