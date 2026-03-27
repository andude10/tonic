<script lang="ts">
    import type { IApi } from "@svar-ui/svelte-grid";
    import { invoke } from "@tauri-apps/api/core";
    import {
        DropDownMenu,
        registerMenuItem,
        type IMenuOptionClick,
    } from "@svar-ui/svelte-menu";
    import InputCell from "./InputCell.svelte";
    import FilterMenuItem from "./FilterMenuItem.svelte";
    import {
        columnLetterToIndex,
        getSheetSharedState,
        isCellData,
        type CellData,
        type FilterOption,
    } from "$lib/sheet/shared";
    import { showError } from "$lib/notice";

    registerMenuItem("filter", FilterMenuItem);

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
    const colIndex = columnLetterToIndex(column.id);

    let focusedThisCell = $derived(
        shared.focusedCell?.row === (row.id as number) - 1 &&
            shared.focusedCell?.col === colIndex,
    );

    const cell = $derived(row[column.id]);
    const displayContent = $derived(
        isCellData(cell) ? cell.computedValue : (cell ?? ""),
    );
    const hasFormula = $derived(isCellData(cell) && cell.isFormula);

    const isTableHeader = $derived(
        shared.tableCellStyles.get(`${row.id},${column.id}`) ===
            "table-header-cell",
    );

    function headerCellId() {
        return {
            row: (row.id as number) - 1,
            col: colIndex,
        };
    }

    let filterOptions: FilterOption[] = $state([]);
    let filterComboOptions: { id: number; label: string }[] = $state([]);

    let filterComboValue = $derived(
        filterOptions.filter((o) => o.selected).map((o) => o.id + 1),
    );

    function handleFilterChange(ev: { value: (string | number)[] }) {
        const next = new Set(ev.value);
        const changed = filterOptions.find(
            (o) => o.selected !== next.has(o.id + 1),
        );
        if (!changed) return;
        changed.selected = !changed.selected;
        invoke("toggle_table_filter", {
            header: headerCellId(),
            filterOptionId: changed.id,
        }).catch((e) => showError(String(e)));
    }

    function handleFilterSelectAll() {
        for (const o of filterOptions) o.selected = true;
        invoke("select_all_table_filters", { header: headerCellId() }).catch(
            (e) => showError(String(e)),
        );
    }

    function handleFilterClear() {
        for (const o of filterOptions) o.selected = false;
        invoke("clear_all_table_filters", { header: headerCellId() }).catch(
            (e) => showError(String(e)),
        );
    }

    let dropdownOptions = $derived([
        { id: "sort-asc", text: "Sort Ascending" },
        { id: "sort-desc", text: "Sort Descending" },
        {
            id: "filter",
            type: "filter",
            css: "filter-option",
            comboOptions: filterComboOptions,
            comboValue: filterComboValue,
            onchange: handleFilterChange,
            onSelectAll: handleFilterSelectAll,
            onClear: handleFilterClear,
        },
    ]);

    function handleDropdownClick(ev: IMenuOptionClick) {
        if (!ev.option) return;
        const desc = ev.option.id === "sort-desc";
        invoke("toggle_table_sort", {
            header: headerCellId(),
            desc,
        }).catch((e) => showError(String(e)));
    }

    function handleDropdownOpen() {
        invoke<FilterOption[]>("get_filter_options_for_table_column", {
            header: headerCellId(),
        })
            .then((opts) => {
                filterOptions = opts;
                filterComboOptions = opts.map((o) => ({
                    id: o.id + 1,
                    label: o.val,
                }));
            })
            .catch((e) => showError(String(e)));
    }

    let editingCellEl: HTMLDivElement | undefined = $state();

    $effect(() => {
        shared.editorInput;
        const cellEl = editingCellEl?.parentElement;
        if (editingCellEl && cellEl) {
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

        <!-- todo: refactor, make state flow (tables) clear -->
        {#if isTableHeader}
            <DropDownMenu
                options={dropdownOptions}
                onclick={handleDropdownClick}
                at="bottom"
            >
                <button
                    class="table-header-btn"
                    aria-label="Column options"
                    onclick={handleDropdownOpen}
                    ><i class="wxi wxi-angle-down"></i></button
                >
            </DropDownMenu>
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
        z-index: 4;
        background: inherit;
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

    .display-cell > :global(span:has(.table-header-btn)) {
        margin-left: auto;
        flex-shrink: 0;
    }

    .table-header-btn {
        padding: 0 0.15em;
        background: none;
        border: none;
        color: rgba(255, 255, 255, 0.35);
        cursor: pointer;
        font-size: 0.875em;
        flex-shrink: 0;
        line-height: 1;
        display: flex;
        align-items: center;
        pointer-events: auto;
    }

    .table-header-btn:hover {
        color: rgba(255, 255, 255, 0.8);
    }
</style>
