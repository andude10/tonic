<script lang="ts">
    import { getContext, tick } from "svelte";
    import type { IApi } from "@svar-ui/svelte-grid";

    let { row, column, api }: { row: any; column: any; api: IApi } = $props();

    const getEditedCell: () => { row: number; column: string } | undefined =
        getContext("editedCell");

    let isEditing = $derived.by(() => {
        const edited = getEditedCell();
        return edited?.row === row.id && edited?.column === column.id;
    });

    let inputNode: HTMLInputElement | undefined = $state();

    $effect(() => {
        if (isEditing) {
            tick().then(() => inputNode?.focus());
        }
    });

    function handleInput() {
        if (inputNode) {
            api.exec("update-cell", {
                id: row.id,
                column: column.id,
                value: inputNode.value,
            });
        }
    }
</script>

{#if isEditing}
    <input
        class="wx-text"
        oninput={handleInput}
        bind:this={inputNode}
        type="text"
        value={row[column.id] ?? ""}
    />
{:else}
    {row[column.id] ?? ""}
{/if}

<style>
    :global(.wx-text) {
        box-sizing: border-box;
        border: none;
        outline: none;
        padding: 0;
        margin: 0;
        font: inherit;
        background: transparent;
        color: inherit;
    }
</style>
