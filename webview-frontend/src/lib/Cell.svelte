<script lang="ts">
    import { getContext, tick } from "svelte";
    import type { IApi } from "@svar-ui/svelte-grid";
    import InputCell from "./InputCell.svelte";
    import type { UICell } from "./data";

    let {
        row,
        column,
        api,
    }: {
        row: any;
        column: any;
        api: IApi;
    } = $props();

    let focusedCell: { ref: UICell | undefined } = getContext("focusedCell");
    let isEditing: { val: boolean } = getContext("isEditing");
    let editorValue: { val: string } = getContext("editorValue");

    let wrapper: HTMLDivElement | undefined = $state();

    let focusedThisCell = $derived(
        focusedCell.ref?.row === row.id &&
            focusedCell.ref?.column === column.id,
    );

    $effect(() => {
        if (focusedThisCell && isEditing.val) {
            tick().then(() => {
                requestAnimationFrame(() => {
                    wrapper
                        ?.querySelector<HTMLInputElement>(".editor")
                        ?.focus();
                });
            });
        }
    });

    // sync cell data into shared editorValue when this cell becomes focused
    $effect(() => {
        if (focusedThisCell) {
            editorValue.val = row[column.id] ?? "";
        }
    });

    function handleInput(value: string) {
        editorValue.val = value;
        api.exec("update-cell", {
            id: row.id,
            column: column.id,
            value,
        });
    }
</script>

{#if isEditing.val && focusedThisCell}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div bind:this={wrapper} onmousedown={(e) => e.stopPropagation()}>
        <InputCell
            class="inline-cell-editor"
            bind:value={editorValue.val}
            oninput={handleInput}
        />
    </div>
{:else}
    <div>{row[column.id] ?? ""}</div>
{/if}

<style>
</style>
