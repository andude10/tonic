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
        color,
        active,
    }: {
        sos: SheetObjectsState;
        bounds: CellRange;
        color: string;
        active: boolean;
    } = $props();

    let rect: PixelRect | null = $state(null);

    export function reposition() {
        rect = cellRangeToPixels(sos, bounds);
    }
</script>

{#if rect}
    <div
        class="ref-overlay"
        class:active
        style:transform={toTranslate3d(rect)}
        style:width="{rect.width}px"
        style:height="{rect.height}px"
        style:color
    ></div>
{/if}

<style>
    .ref-overlay {
        position: absolute;
        border: 2px solid currentColor;
        border-radius: 1px;
        will-change: transform, width, height;
        pointer-events: none;
        --transition-base:
            transform 30ms cubic-bezier(0, 0, 0.2, 1),
            width 30ms cubic-bezier(0, 0, 0.2, 1),
            height 30ms cubic-bezier(0, 0, 0.2, 1);
        transition:
            var(--transition-base),
            color 30ms cubic-bezier(0, 0, 0.2, 1);
    }

    .ref-overlay::before {
        content: "";
        position: absolute;
        inset: -2px;
        border: 1px solid currentColor;
        border-radius: 1px;
        opacity: 0.5;
    }

    .ref-overlay.active {
        background-color: color-mix(in srgb, currentColor 6%, transparent);
        border-color: transparent;
    }

    .ref-overlay.active::before {
        opacity: 0;
    }

    .ref-overlay.active::after {
        content: "";
        position: absolute;
        inset: 0;
        pointer-events: none;
        background:
            linear-gradient(90deg, currentColor 50%, transparent 0) 0 0 / 12px
                2px repeat-x,
            linear-gradient(90deg, currentColor 50%, transparent 0) 0 100% /
                12px 2px repeat-x,
            linear-gradient(0deg, currentColor 50%, transparent 0) 0 0 / 2px
                12px repeat-y,
            linear-gradient(0deg, currentColor 50%, transparent 0) 100% 0 / 2px
                12px repeat-y;
        will-change: background-position;
        animation: ants 0.6s linear infinite;
    }

    @keyframes ants {
        100% {
            background-position:
                12px 0,
                -12px 100%,
                0 -12px,
                100% 12px;
        }
    }
</style>
