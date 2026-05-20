<script lang="ts">
    import {
        cellRangeToPixels,
        toTranslate3d,
        type CellRange,
        type SheetObjectsState,
        type PixelRect,
    } from "./Overlays.svelte";
    import { getSheetSharedState } from "../shared";

    let {
        sos,
        bounds,
        visible,
        isFilling,
        showBorder = true,
        showBackground = false,
        zIndex = 6,
        onfillstart,
    }: {
        sos: SheetObjectsState;
        bounds: CellRange | null;
        visible: boolean;
        isFilling: boolean;
        showBorder?: boolean;
        showBackground?: boolean;
        zIndex?: number;
        onfillstart: (ev: MouseEvent) => void;
    } = $props();

    const shared = getSheetSharedState();

    // derive expanded cell's extra dimensions directly from shared state,
    // so FocusOverlay wraps the expanded cell correctly
    let extraWidth = $derived.by(() => {
        const fc = shared.focusedCell;
        return fc
            ? (shared.expandedCells.get(`${fc.row},${fc.col}`)?.extraWidth ?? 0)
            : 0;
    });
    let extraHeight = $derived.by(() => {
        const fc = shared.focusedCell;
        return fc
            ? (shared.expandedCells.get(`${fc.row},${fc.col}`)?.extraHeight ??
                  0)
            : 0;
    });

    let rect: PixelRect | null = $state(null);

    export function reposition() {
        rect = bounds ? cellRangeToPixels(sos, bounds) : null;
    }

    let show = $derived(visible && !!rect);
    let showResizeHandles = $derived(shared.expandModeActive && showBorder);

    // hide corner handle when it would visually overlap with right or bottom handle
    // (happens when the cell is too small, less than ~30px in either dimension)
    let totalWidth = $derived(
        rect ? (rect as PixelRect).width + extraWidth : 0,
    );
    let totalHeight = $derived(
        rect ? (rect as PixelRect).height + extraHeight : 0,
    );
    let showCornerHandle = $derived(totalWidth > 30 && totalHeight > 30);

    // multi-cell drag: include selected cells in the same column/row as the focused cell.
    // all drag state is local to this closure, no shared state needed.
    function startDrag(edge: "right" | "bottom" | "both", ev: MouseEvent) {
        ev.stopPropagation();
        ev.preventDefault();
        const fc = shared.focusedCell;
        if (!fc) return;
        const cellKey = `${fc.row},${fc.col}`;
        const entry = shared.expandedCells.get(cellKey) ?? {
            extraWidth: 0,
            extraHeight: 0,
        };
        const startX = ev.clientX;
        const startY = ev.clientY;
        const startExtraW = entry.extraWidth;
        const startExtraH = entry.extraHeight;

        // collect all cells affected by this drag (multi-cell selection support)
        const affectedKeys = [cellKey];
        for (const sel of shared.selections) {
            if (
                (edge === "right" || edge === "both") &&
                fc.col >= sel.minC &&
                fc.col <= sel.maxC
            )
                for (let r = sel.minR; r <= sel.maxR; r++) {
                    const k = `${r},${fc.col}`;
                    if (!affectedKeys.includes(k)) affectedKeys.push(k);
                }
            if (
                (edge === "bottom" || edge === "both") &&
                fc.row >= sel.minR &&
                fc.row <= sel.maxR
            )
                for (let c = sel.minC; c <= sel.maxC; c++) {
                    const k = `${fc.row},${c}`;
                    if (!affectedKeys.includes(k)) affectedKeys.push(k);
                }
        }

        // use window-level listeners so dragging works even when cursor leaves the overlay
        function onMove(e: MouseEvent) {
            const dx = e.clientX - startX;
            const dy = e.clientY - startY;
            const next = new Map(shared.expandedCells);
            for (const key of affectedKeys) {
                const cur = next.get(key) ?? { extraWidth: 0, extraHeight: 0 };
                next.set(key, {
                    extraWidth:
                        edge === "bottom"
                            ? cur.extraWidth
                            : Math.max(0, startExtraW + dx),
                    extraHeight:
                        edge === "right"
                            ? cur.extraHeight
                            : Math.max(0, startExtraH + dy),
                });
            }
            shared.expandedCells = next;
        }

        function onUp() {
            window.removeEventListener("mousemove", onMove);
            window.removeEventListener("mouseup", onUp);
            // remove entries that collapsed back to zero
            const next = new Map(shared.expandedCells);
            for (const [k, v] of next) {
                if (v.extraWidth <= 0 && v.extraHeight <= 0) next.delete(k);
            }
            shared.expandedCells = next;
        }

        window.addEventListener("mousemove", onMove);
        window.addEventListener("mouseup", onUp);
    }
</script>

{#if show}
    <div
        class="focus-overlay"
        style:transform={toTranslate3d(rect!)}
        style:width="{Math.max(
            rect!.width + extraWidth,
            shared.isEditing ? shared.editorInputWidth : 0,
        )}px"
        style:height="{rect!.height + extraHeight}px"
        style:border-color={showBorder
            ? "var(--wx-color-primary)"
            : "transparent"}
        style:background={showBackground
            ? "color-mix(in srgb, var(--wx-color-primary), transparent 88%)"
            : "transparent"}
        style:z-index={zIndex}
    >
        <!-- fill handle: small square at bottom-right for cell fill drag (hidden during resize mode) -->
        {#if showBorder && !showResizeHandles}
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div
                class="fill-handle"
                class:filling={isFilling}
                onmousedown={onfillstart}
            ></div>
        {/if}

        <!-- resize handles: appear when expand mode is active (double-tap Shift) -->
        <!-- arrows sit centered on the focus border so half is inside, half outside -->
        {#if showResizeHandles}
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div
                class="resize-handle resize-right"
                onmousedown={(ev) => startDrag("right", ev)}
            >
                <svg class="resize-icon" viewBox="0 -0.5 17 17"
                    ><path
                        d="M5,9 L13.066,9 L13.066,11.8638731 L16.916,8.0438731 L13.066,4.0308731 L13,7 L5,7 L4.886,4.0308731 L1.03,8.0688731 L4.886,11.9348731 L5,9 Z"
                        fill="currentColor"
                    /></svg
                >
            </div>
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div
                class="resize-handle resize-bottom"
                onmousedown={(ev) => startDrag("bottom", ev)}
            >
                <svg
                    class="resize-icon resize-icon-rotated"
                    viewBox="0 -0.5 17 17"
                    ><path
                        d="M5,9 L13.066,9 L13.066,11.8638731 L16.916,8.0438731 L13.066,4.0308731 L13,7 L5,7 L4.886,4.0308731 L1.03,8.0688731 L4.886,11.9348731 L5,9 Z"
                        fill="currentColor"
                    /></svg
                >
            </div>
            <!-- corner handle: hidden when cell is too small to avoid overlap with right/bottom handles -->
            {#if showCornerHandle}
                <!-- svelte-ignore a11y_no_static_element_interactions -->
                <div
                    class="resize-handle resize-corner"
                    onmousedown={(ev) => startDrag("both", ev)}
                >
                    <svg
                        class="resize-icon resize-icon-corner"
                        viewBox="0 0 16 16"
                        ><path
                            d="M0.648,15.938 C0.49,15.938 0.333,15.877 0.212,15.757 C-0.03,15.517 -0.03,15.126 0.212,14.886 L14.946,0.18 C15.186,-0.06 15.577,-0.06 15.817,0.18 C16.059,0.421 16.059,0.812 15.817,1.052 L1.085,15.757 C0.965,15.877 0.808,15.938 0.648,15.938 Z M5.652,16.01 C5.491,16.01 5.332,15.948 5.211,15.825 C4.967,15.584 4.967,15.187 5.211,14.943 L14.934,5.22 C15.176,4.975 15.573,4.975 15.815,5.22 C16.061,5.462 16.061,5.858 15.815,6.103 L6.092,15.825 C5.971,15.948 5.812,16.01 5.652,16.01 Z M10.633,15.985 C10.48,15.985 10.324,15.925 10.207,15.807 C9.973,15.572 9.973,15.19 10.207,14.954 L14.959,10.202 C15.195,9.965 15.576,9.965 15.813,10.202 C16.047,10.437 16.047,10.819 15.813,11.055 L11.06,15.807 C10.942,15.926 10.787,15.985 10.633,15.985 Z"
                            fill="currentColor"
                        /></svg
                    >
                </div>
            {/if}
        {/if}
    </div>
{/if}

<style>
    .focus-overlay {
        position: absolute;
        border: 2px solid transparent;
        border-radius: 1px;
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
        width: 8px;
        height: 8px;
        background: var(--wx-color-primary);
        border: 1px solid var(--wx-background);
        border-radius: 1px;
        cursor: crosshair;
        pointer-events: auto;
        z-index: 10;
    }

    .fill-handle.filling {
        width: 10px;
        height: 10px;
        bottom: -5px;
        right: -5px;
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

    .resize-handle {
        position: absolute;
        pointer-events: auto;
        display: flex;
        align-items: center;
        justify-content: center;
        color: var(--wx-color-primary);
    }

    /* right handle: centered on the right border edge (half inside, half outside) */
    .resize-right {
        top: 50%;
        right: -8px;
        transform: translateY(-50%);
        width: 18px;
        height: 18px;
        cursor: ew-resize;
    }

    /* bottom handle: centered on the bottom border edge */
    .resize-bottom {
        bottom: -8px;
        left: 50%;
        transform: translateX(-50%);
        width: 18px;
        height: 18px;
        cursor: ns-resize;
    }

    /* corner handle: inside the focus border, bottom-right */
    .resize-corner {
        bottom: 0px;
        right: 0px;
        width: 14px;
        height: 14px;
        cursor: nwse-resize;
    }

    .resize-icon {
        width: 14px;
        height: 14px;
    }

    .resize-icon-rotated {
        transform: rotate(90deg);
    }

    .resize-icon-corner {
        width: 10px;
        height: 10px;
    }
</style>
