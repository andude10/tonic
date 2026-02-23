<script lang="ts">
    import { tick } from "svelte";
    import type { IApi } from "@svar-ui/svelte-grid";
    import InputCell from "./InputCell.svelte";
    import {
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

    // todo: move isFormula to shared state
    let isFormula = $derived(shared.editorInput.startsWith("="));

    let wrapper: HTMLDivElement | undefined = $state();

    let focusedThisCell = $derived(
        shared.focusedCell?.row === row.id &&
            shared.focusedCell?.column === column.id,
    );

    $effect(() => {
        if (focusedThisCell && shared.isEditing) {
            tick().then(() => {
                requestAnimationFrame(() => {
                    wrapper
                        ?.querySelector<HTMLInputElement>(".editor")
                        ?.focus();
                });
            });
        }
    });

    const displayContent = $derived.by(() => {
        const cell = row[column.id];
        if (isCellData(cell)) {
            return cell.computedValue;
        }
        return cell;
    });
</script>

{#if shared.isEditing && focusedThisCell}
    <div bind:this={wrapper}>
        <InputCell {isFormula} bind:value={shared.editorInput} />
    </div>
{:else}
    <div>
        {displayContent}
    </div>
{/if}

<style>
</style>
