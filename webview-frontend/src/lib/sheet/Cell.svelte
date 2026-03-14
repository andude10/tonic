<script lang="ts">
    import { tick } from "svelte";
    import type { IApi } from "@svar-ui/svelte-grid";
    import InputCell from "./InputCell.svelte";
    import {
        columnLetterToIndex,
        getSheetSharedState,
        isCellData,
        type CellData,
    } from "$lib/sheet/shared";

    let {
        row,
        column,
        api,
    }: {
        row: Record<string, CellData | number>;
        column: any;
        api: IApi;
    } = $props();

    const shared = getSheetSharedState();

    let focusedThisCell = $derived(
        shared.focusedCell?.row === (row.id as number) - 1 &&
            shared.focusedCell?.col === columnLetterToIndex(column.id),
    );

    const displayContent = $derived.by(() => {
        const cell = row[column.id];
        if (isCellData(cell)) return cell.computedValue;
        return cell ?? "";
    });

    // todo: simplify
    const hasFormula = $derived.by(() => {
        const cell = row[column.id];
        return isCellData(cell) && cell.isFormula;
    });

    let editingCellEl: HTMLDivElement | undefined = $state();

    $effect(() => {
        shared.editorInput;
        if (editingCellEl) {
            const cellEl = editingCellEl.parentElement!;
            const padding =
                parseFloat(getComputedStyle(cellEl).paddingLeft) * 2;
            shared.editorInputWidth = editingCellEl.offsetWidth + padding;
        }
    });
</script>

{#if shared.isEditing && focusedThisCell}
    <div
        class="editing-cell"
        class:formula={shared.editorInputIsFormula}
        data-value={shared.editorInput + "\u00a0"}
        bind:this={editingCellEl}
    >
        <InputCell
            editorInputIsFormula={shared.editorInputIsFormula}
            bind:editorInput={shared.editorInput}
            editorInputHtml={shared.editorInputHtml}
        />
    </div>
{:else}
    <div class="display-cell">
        {displayContent}
        {#if hasFormula}
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
    </div>
{/if}

<style>
    :global {
        .wx-cell:has(> .editing-cell) {
            overflow: visible !important;
            padding-top: 0 !important;
            padding-bottom: 0 !important;
        }
    }

    .editing-cell {
        display: inline-grid;
        align-items: center;
        min-width: 100%;
        height: 100%;
        position: relative;
        z-index: 6;
    }

    .editing-cell::after {
        content: attr(data-value);
        visibility: hidden;
        white-space: pre;
        font: inherit;
        padding: inherit;
        grid-area: 1 / 1;
    }

    .editing-cell.formula::after {
        font-family: "JetBrains Mono", monospace;
    }

    .editing-cell > :global(*) {
        grid-area: 1 / 1;
    }

    .display-cell {
        display: flex;
        align-items: center;
        width: 100%;
        height: 100%;
    }

    .formula-indicator {
        margin-left: auto;
        width: 1.3em;
        height: 1.3em;
        padding: 0.15em;
        flex-shrink: 0;
        color: rgba(255, 255, 255, 0.15);
        border: var(--wx-border);
    }
</style>
