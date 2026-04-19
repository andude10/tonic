<script lang="ts">
    import {
        cellRangeToPixels,
        toTranslate3d,
        type SheetObjectsState,
        type PixelRect,
    } from "./Overlays.svelte";
    import InputCell from "../InputCell.svelte";
    import { getSheetSharedState, columnIndexToLetter } from "../shared";

    let { sos, cellKey }: { sos: SheetObjectsState; cellKey: string } =
        $props();

    const shared = getSheetSharedState();
    let [row, col] = $derived(cellKey.split(",").map(Number));
    let colLetter = $derived(columnIndexToLetter(col));

    let isFocused = $derived(
        shared.focusedCell?.row === row && shared.focusedCell?.col === col,
    );
    let isCellEditing = $derived(isFocused && shared.isEditing);
    let expansion = $derived(
        shared.expandedCells.get(cellKey) ?? { extraWidth: 0, extraHeight: 0 },
    );

    // cellStyles uses 1-indexed SVAR row id + column letter as key
    let cellStyle = $derived(
        shared.cellStyles.get(`${row + 1},${colLetter}`) ?? "",
    );
    let isFormula = $derived(shared.getCellIsFormula(row, col));

    let rect: PixelRect | null = $state(null);

    export function reposition() {
        rect = cellRangeToPixels(sos, {
            minR: row,
            maxR: row,
            minC: col,
            maxC: col,
        });
    }
</script>

{#if rect}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
        class="expanded-cell {cellStyle}"
        style:transform={toTranslate3d(rect)}
        style:width="{rect.width + expansion.extraWidth}px"
        style:height="{rect.height + expansion.extraHeight}px"
        onmousedown={(ev) => {
            ev.stopPropagation();
            if (isFocused && !shared.isEditing) {
                shared.isEditing = true;
                const container = ev.currentTarget as HTMLElement;
                requestAnimationFrame(() =>
                    requestAnimationFrame(() => {
                        container
                            ?.querySelector<HTMLInputElement>(".editor")
                            ?.focus();
                    }),
                );
            }
            shared.focusedCell = { row, col };
            shared.hoveredCell = { row, col };
            shared.selections = [];
        }}
    >
        {#if isCellEditing}
            <InputCell
                editorInputIsFormula={shared.editorInputIsFormula}
                bind:editorInput={shared.editorInput}
                editorInputHtml={shared.editorInputHtml}
                onkeydown={(ev) => {
                    if (ev.key === "Enter" || ev.key === "Escape") {
                        shared.commitEdit();
                        shared.isEditing = false;
                        document
                            .querySelector<HTMLElement>(".grid-wrapper")
                            ?.focus();
                    }
                }}
            />
        {:else}
            <span class="cell-text">{shared.getCellDisplayValue(row, col)}</span
            >
            {#if isFormula}
                <svg
                    class="formula-indicator"
                    viewBox="0 0 24 24"
                    xmlns="http://www.w3.org/2000/svg"
                    ><path
                        fill="currentColor"
                        d="M18 7h-12c-1.104 0-2 .896-2 2s.896 2 2 2h12c1.104 0 2-.896 2-2s-.896-2-2-2zM18 14h-12c-1.104 0-2 .896-2 2s.896 2 2 2h12c1.104 0 2-.896 2-2s-.896-2-2-2z"
                    /></svg
                >
            {/if}
        {/if}
    </div>
{/if}

<style>
    .expanded-cell {
        position: absolute;
        background: var(--wx-background);
        border: var(--wx-table-cell-border);
        box-shadow: 0 2px 12px rgba(0, 0, 0, 0.15);
        box-sizing: border-box;
        z-index: 4;
        pointer-events: auto;
        will-change: transform, width, height;
        overflow: hidden;
        padding: 0 6px;
        display: flex;
        align-items: center;
        font: inherit;
    }

    /* table cell styles - same classes as Sheet.svelte's global styles */
    .expanded-cell:global(.table-header-cell) {
        background: var(--wx-table-header-background);
        font-weight: 500;
        color: #b0b0b0;
    }
    .expanded-cell:global(.table-row-even) {
        background: var(--tonic-row-even-bg);
    }
    .expanded-cell:global(.table-row-odd) {
        background: var(--tonic-row-odd-bg);
    }

    .cell-text {
        overflow: hidden;
        text-overflow: ellipsis;
        flex: 1;
        min-width: 0;
    }

    .formula-indicator {
        margin-left: auto;
        width: 1.3em;
        height: 1.3em;
        padding: 0.15em;
        flex-shrink: 0;
        color: var(--tonic-text-muted);
        border: var(--wx-border-light);
        border-radius: 1px;
    }
</style>
