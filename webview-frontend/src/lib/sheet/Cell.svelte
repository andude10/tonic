<script lang="ts">
    import type { IApi } from "@svar-ui/svelte-grid";
    import { invoke } from "@tauri-apps/api/core";
    import {
        DropDownMenu,
        registerMenuItem,
        type IMenuOptionClick,
    } from "@svar-ui/svelte-menu";
    import { Popup } from "@svar-ui/svelte-core";
    import InputCell from "./InputCell.svelte";
    import FilterMenuItem from "./FilterMenuItem.svelte";
    import {
        columnLetterToIndex,
        getSheetSharedState,
        isCellData,
        type CellData,
        type CellFormatting,
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
    let colIndex = $derived(columnLetterToIndex(column.id));

    let focusedThisCell = $derived(
        shared.focusedCell?.row === (row.id as number) - 1 &&
            shared.focusedCell?.col === colIndex,
    );

    // Hide cell content when an ExpandedCellOverlay renders this cell
    let expandKey = $derived(`${(row.id as number) - 1},${colIndex}`);
    let hasExpandOverlay = $derived(
        shared.expandedCells.has(expandKey) ||
            (shared.expandModeActive && focusedThisCell),
    );

    const cell = $derived(row[column.id]);
    const displayContent = $derived(
        isCellData(cell) ? cell.computedValue : (cell ?? ""),
    );
    const hasFormula = $derived(isCellData(cell) && cell.isFormula);
    const isPending = $derived(isCellData(cell) && !!cell.isPending);
    const isError = $derived(isCellData(cell) && !!cell.isError);
    const errorMessage = $derived(
        isCellData(cell) && cell.errorMessage ? cell.errorMessage : "",
    );
    const formatting = $derived(isCellData(cell) ? cell.formatting : {});
    const formattingStyle = $derived(cellFormattingStyle(formatting));

    function cellFormattingStyle(formatting: CellFormatting): string {
        let style = "";
        // default bold should inherit from table/header styles.
        if (formatting.bold) style += "font-weight: 700;";
        // default italic should inherit from table/header styles.
        if (formatting.italic) style += "font-style: italic;";
        // default decoration should not override formulas/errors.
        if (formatting.strikethrough) style += "text-decoration: line-through;";
        // default color should keep theme/table colors.
        if (formatting.textColor) style += `color: ${formatting.textColor};`;
        return style;
    }

    const isTableHeader = $derived(
        shared.cellStyles.get(`${row.id},${column.id}`) === "table-header-cell",
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
    let errorPopupOpen = $state(false);
    let errorIndicatorEl: HTMLButtonElement | undefined = $state();

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

{#if hasExpandOverlay}
    <!-- ExpandedCellOverlay renders this cell -->
{:else if shared.isEditing && focusedThisCell}
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
{:else if isPending}
    <div class="display-cell pending-cell" style={formattingStyle}>
        <span class="pending-spinner" aria-label="Calculating"></span>
    </div>
{:else}
    <div class="display-cell" style={formattingStyle}>
        {displayContent}
        {#if isError}
            <button
                class="cell-indicator error-indicator"
                aria-label="Error details"
                bind:this={errorIndicatorEl}
                onclick={() => {
                    errorPopupOpen = !errorPopupOpen;
                }}
            >
                <svg viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg"
                    ><path
                        fill="none"
                        stroke="currentColor"
                        stroke-width="2"
                        stroke-linecap="round"
                        d="M7 7l10 10M17 7L7 17"
                    /></svg
                >
            </button>
            {#if errorPopupOpen}
                <Popup
                    oncancel={() => {
                        errorPopupOpen = false;
                    }}
                    at="bottom"
                    parent={errorIndicatorEl}
                >
                    <div class="error-popup">
                        <pre>{errorMessage}</pre>
                        <button
                            class="error-copy-btn"
                            onclick={() =>
                                navigator.clipboard.writeText(errorMessage)}
                            >Copy</button
                        >
                    </div>
                </Popup>
            {/if}
        {:else if hasFormula}
            <button
                class="cell-indicator formula-indicator"
                aria-label="Formula"
            >
                <svg viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg"
                    ><path
                        fill="none"
                        stroke="currentColor"
                        stroke-width="2"
                        stroke-linecap="round"
                        d="M5 9h14M5 15h14"
                    /></svg
                >
            </button>
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

    .pending-cell {
        justify-content: center;
        opacity: 0.65;
    }

    .pending-spinner {
        width: 0.85em;
        height: 0.85em;
        border: 2px solid currentColor;
        border-right-color: transparent;
        border-radius: 50%;
        animation: pending-spin 0.75s linear infinite;
    }

    @keyframes pending-spin {
        to {
            transform: rotate(360deg);
        }
    }

    .cell-indicator {
        all: unset;
        box-sizing: border-box;
        margin-left: auto;
        width: 1.3em;
        height: 1.3em;
        padding: 0.15em;
        flex-shrink: 0;
        border-radius: 1px;
        display: flex;
        align-items: center;
        justify-content: center;
        pointer-events: auto;
    }

    .cell-indicator > :global(svg) {
        width: 100%;
        height: 100%;
    }

    .formula-indicator {
        color: var(--tonic-text-muted);
        border: var(--wx-border-light);
    }

    .error-indicator {
        cursor: pointer;
        color: #f87171;
        background: rgba(248, 113, 113, 0.12);
        border: 1px solid rgba(248, 113, 113, 0.3);
    }

    .error-indicator:hover {
        background: rgba(248, 113, 113, 0.22);
    }

    :global(.error-popup) {
        padding: 6px 10px;
        font-size: 11px;
        font-family: "JetBrains Mono", monospace;
        width: max-content;
    }

    :global(.error-popup pre) {
        margin: 0;
        white-space: pre;
        font: inherit;
    }

    :global(.error-popup .error-copy-btn) {
        all: unset;
        display: block;
        margin-top: 4px;
        padding: 2px 8px;
        font-size: 10px;
        cursor: pointer;
        border-radius: 2px;
        background: var(--tonic-cell-tint);
        color: var(--tonic-text-dim);
        transition: background 100ms ease;
    }

    :global(.error-popup .error-copy-btn:hover) {
        background: var(--tonic-cell-tint-strong);
    }

    .display-cell > :global(span:has(.table-header-btn)),
    .display-cell > :global(span:has(.error-indicator)) {
        margin-left: auto;
        flex-shrink: 0;
    }

    .table-header-btn {
        padding: 0 0.15em;
        background: none;
        border: none;
        color: var(--tonic-text-muted);
        cursor: pointer;
        font-size: 0.875em;
        flex-shrink: 0;
        line-height: 1;
        display: flex;
        align-items: center;
        pointer-events: auto;
        transition: color 100ms ease;
    }

    .table-header-btn:hover {
        color: var(--tonic-text-dim);
    }
</style>
