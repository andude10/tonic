<script lang="ts">
    import { MultiCombo } from "@svar-ui/svelte-core";

    let { option }: { option?: any } = $props();
</script>

<div
    role="toolbar"
    tabindex="-1"
    class="filter-menu-item"
    onclick={(e) => e.preventDefault()}
    onkeydown={(e) => e.stopPropagation()}
    onmousedown={(e) => e.stopPropagation()}
>
    <MultiCombo
        checkboxes={true}
        options={option.comboOptions}
        value={option.comboValue}
        onchange={option.onchange}
        placeholder="Filter..."
    />
</div>

<style>
    .filter-menu-item {
        flex: 1;
        min-width: 0;
    }

    .filter-menu-item :global(.wx-tags) {
        display: none;
    }
</style>
