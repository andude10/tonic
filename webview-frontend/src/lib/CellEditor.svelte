<script lang="ts">
    import { getContext, tick } from "svelte";
    import type { IApi } from "@svar-ui/svelte-grid";
    import FormulaInput from "./FormulaInput.svelte";

    let { row, column, api }: { row: any; column: any; api: IApi } = $props();

    const getEditedCell: () => { row: number; column: string } | undefined =
        getContext("editedCell");

    let isEditing = $derived.by(() => {
        const edited = getEditedCell();
        return edited?.row === row.id && edited?.column === column.id;
    });

    let wrapper: HTMLDivElement | undefined = $state();

    $effect(() => {
        if (isEditing) {
            tick().then(() => {
                const input =
                    wrapper?.querySelector<HTMLInputElement>(".editor");
                input?.focus();
            });
        }
    });

    function handleChange(value: string) {
        api.exec("update-cell", {
            id: row.id,
            column: column.id,
            value,
        });
    }
</script>

{#if isEditing}
    <div bind:this={wrapper} class="cell-editor-wrap">
        <FormulaInput
            value={row[column.id] ?? ""}
            onchange={handleChange}
            class="cell-editor"
        />
    </div>
{:else}
    {row[column.id] ?? ""}
{/if}

<style>
    .cell-editor-wrap {
        width: 100%;
        height: 100%;
    }
</style>
