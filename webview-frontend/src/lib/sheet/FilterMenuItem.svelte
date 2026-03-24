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
    <div class="filter-actions">
        <button class="filter-link" onclick={() => option.onSelectAll?.()}
            >Select all</button
        >
        <button class="filter-link" onclick={() => option.onClear?.()}
            >Clear</button
        >
    </div>
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

    .filter-actions {
        display: flex;
        gap: 8px;
        padding: 0 2px 4px;
    }

    .filter-link {
        background: none;
        border: none;
        padding: 0;
        font-size: 12px;
        color: var(--wx-color-primary, #7e5dab);
        cursor: pointer;
        opacity: 0.8;
    }

    .filter-link:hover {
        opacity: 1;
        text-decoration: underline;
    }
</style>
