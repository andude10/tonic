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
</script>

{#if shared.isEditing && focusedThisCell}
    <div>
        <InputCell
            editorInputIsFormula={shared.editorInputIsFormula}
            bind:editorInput={shared.editorInput}
            editorInputHtml={shared.editorInputHtml}
        />
    </div>
{:else}
    <div>
        {displayContent}
    </div>
{/if}

<style>
</style>
