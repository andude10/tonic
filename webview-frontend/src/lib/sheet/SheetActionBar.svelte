<script lang="ts">
    import { invoke } from "@tauri-apps/api/core";
    import {
        getSheetSharedState,
        type CellFormatting,
        type CellId,
    } from "$lib/sheet/shared";
    import { showError } from "$lib/notice";

    type ToggleFormat = "bold" | "italic" | "strikethrough";

    const shared = getSheetSharedState();
    const DEFAULT_TEXT_COLOR = "#d32f2f";

    let focusedKey = $state("");
    let optimisticFormatting: CellFormatting | null = $state(null);

    $effect(() => {
        const nextKey = shared.focusedCell
            ? `${shared.focusedCell.row}:${shared.focusedCell.col}`
            : "";
        // optimistic toolbar state should not leak to another focused cell.
        if (nextKey !== focusedKey) {
            focusedKey = nextKey;
            optimisticFormatting = null;
        }
    });

    let focusedFormatting = $derived.by(() => {
        // backend refresh is async, so keep button state responsive.
        if (optimisticFormatting) return optimisticFormatting;
        const cell = shared.focusedCell;
        return cell ? shared.getCellFormatting(cell.row, cell.col) : {};
    });
    let textColor = $derived(focusedFormatting.textColor ?? DEFAULT_TEXT_COLOR);

    function targetCells(): CellId[] {
        const cells = new Map<string, CellId>();
        // explicit selections should receive toolbar changes together.
        for (const range of shared.selections) {
            for (let row = range.minR; row <= range.maxR; row++) {
                for (let col = range.minC; col <= range.maxC; col++) {
                    cells.set(`${row}:${col}`, { row, col });
                }
            }
        }
        // focused cell is the fallback when nothing is selected.
        if (!cells.size && shared.focusedCell) {
            cells.set(
                `${shared.focusedCell.row}:${shared.focusedCell.col}`,
                shared.focusedCell,
            );
        }
        return [...cells.values()];
    }

    function applyFormatting(
        command: string,
        value: boolean | string,
        optimistic: CellFormatting,
    ) {
        const cells = targetCells();
        // focus can disappear between click and handler execution.
        if (!cells.length) return;

        shared.commitEdit();
        shared.isEditing = false;
        optimisticFormatting = { ...focusedFormatting, ...optimistic };
        invoke(command, { cells, value }).catch((e) => showError(String(e)));
    }

    function toggleFormatting(command: string, key: ToggleFormat) {
        const value = !focusedFormatting[key];
        applyFormatting(command, value, { [key]: value });
    }

    function handleTextColorChange(ev: Event) {
        const input = ev.currentTarget as HTMLInputElement;
        applyFormatting("set_cells_text_color", input.value, {
            textColor: input.value,
        });
    }
</script>

<div class="action-bar">
    <button
        class:active={!!focusedFormatting.bold}
        disabled={!shared.focusedCell}
        aria-label="Bold"
        onclick={() => toggleFormatting("set_cells_bold", "bold")}
    >
        <b>B</b>
    </button>
    <button
        class:active={!!focusedFormatting.italic}
        disabled={!shared.focusedCell}
        aria-label="Italic"
        onclick={() => toggleFormatting("set_cells_italic", "italic")}
    >
        <i>I</i>
    </button>
    <button
        class:active={!!focusedFormatting.strikethrough}
        disabled={!shared.focusedCell}
        aria-label="Strikethrough"
        onclick={() =>
            toggleFormatting("set_cells_strikethrough", "strikethrough")}
    >
        <span class="strike">S</span>
    </button>
    <label
        class="color-button"
        class:active={!!focusedFormatting.textColor}
        class:disabled={!shared.focusedCell}
        aria-label="Text color"
        title="Text color"
    >
        <span style:color={textColor}>A</span>
        <input
            type="color"
            value={textColor}
            disabled={!shared.focusedCell}
            onchange={handleTextColorChange}
        />
    </label>
</div>

<style>
    .action-bar {
        display: flex;
        align-items: center;
        gap: 0.25rem;
        height: 2rem;
        padding: 0 0.5rem;
        user-select: none;
        border-bottom: var(--wx-border);
    }

    button,
    .color-button {
        all: unset;
        box-sizing: border-box;
        width: 1.625rem;
        height: 1.625rem;
        display: flex;
        align-items: center;
        justify-content: center;
        border-radius: 0.1875rem;
        color: var(--tonic-text-dim);
        cursor: pointer;
        position: relative;
    }

    button:hover:not(:disabled),
    .color-button:hover:not(.disabled),
    .active {
        background: var(--tonic-cell-tint);
        color: var(--wx-color-font);
    }

    button:disabled,
    .disabled {
        opacity: 0.45;
        cursor: default;
    }

    .strike {
        text-decoration: line-through;
    }

    input {
        position: absolute;
        inset: 0;
        opacity: 0;
        cursor: pointer;
    }

    input:disabled {
        cursor: default;
    }
</style>
