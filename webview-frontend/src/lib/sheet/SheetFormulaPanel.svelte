<script lang="ts">
    import { invoke } from "@tauri-apps/api/core";
    import InputCell from "./InputCell.svelte";
    import { getSheetSharedState } from "$lib/sheet/shared";
    import { showError } from "$lib/notice";

    const shared = getSheetSharedState();

    let cellNameInput = $state("");
    let cellNameTarget: { row: number; col: number } | null = null;

    $effect(() => {
        if (!shared.isEditingCellName) cellNameInput = shared.cellName;
    });

    function handleCellNameFocus(): void {
        shared.commitEdit();
        shared.isEditing = false;
        shared.isEditingCellName = true;
        cellNameTarget = shared.focusedCell ? { ...shared.focusedCell } : null;
    }

    async function handleCellNameBlur(): Promise<void> {
        shared.isEditingCellName = false;
        const target = cellNameTarget;
        cellNameTarget = null;
        if (!target) return;
        const name = cellNameInput.trim();
        if (name === shared.cellName) return;
        try {
            await invoke("rename_cell", { cellId: target, name });
        } catch (e) {
            showError("Rename failed: " + e);
        }
    }

    function handleCellNameKeyDown(ev: KeyboardEvent): void {
        if (ev.key === "Enter" || ev.key === "Escape") {
            if (ev.key === "Escape") cellNameInput = shared.cellName;
            document.querySelector<HTMLElement>(".grid-wrapper")?.focus();
        }
    }

    function handleInput(ev: Event): void {
        shared.isEditing = true;
    }

    function handleFocus(ev: FocusEvent): void {
        if (shared.focusedCell) {
            shared.isEditing = true;
        }
    }

    function handleKeyDown(ev: KeyboardEvent): void {
        // on Enter or Escape, commit change and move focus to sheet
        if (ev.key === "Enter" || ev.key === "Escape") {
            shared.commitEdit();
            shared.isEditing = false;
            document.querySelector<HTMLElement>(".grid-wrapper")?.focus();
        }
    }
</script>

<div class="panel">
    <div class="panel-content">
        <div class="cell-ref-group">
            <input
                id="cell-ref"
                type="text"
                class="cell-ref-input"
                bind:value={cellNameInput}
                onfocus={handleCellNameFocus}
                onblur={handleCellNameBlur}
                onkeydown={handleCellNameKeyDown}
                placeholder="—"
            />
        </div>

        <div class="content-group" onfocusin={handleFocus}>
            <div class="formula-label">ƒ(x)</div>

            <InputCell
                editorInputIsFormula={shared.editorInputIsFormula}
                bind:editorInput={shared.editorInput}
                editorInputHtml={shared.editorInputHtml}
                disabled={!shared.focusedCell}
                class="top-panel-editor"
                oninput={handleInput}
                onkeydown={handleKeyDown}
            />
        </div>
    </div>
</div>

<style>
    .panel {
        display: flex;
        align-items: center;
        height: 1.875rem;
        padding: 0 0.25rem;
        user-select: none;
        border-bottom: var(--wx-border);
    }

    .panel-content {
        display: flex;
        align-items: center;
        gap: 0.625rem;
        width: 100%;
        height: 100%;
    }

    .cell-ref-group {
        display: flex;
        align-items: center;
        border-right: var(--wx-border);
        margin: -1px 0.25rem -1px 0;
        padding: 1px 0.375rem 1px 0;
    }

    .cell-ref-input {
        width: 4.5rem;
        height: 1.5rem;
        padding-left: 0.8rem;
        padding-right: 0.8rem;
        border: none;
        background: transparent;
        color: var(--tonic-text-dim);
        font-family: "JetBrains Mono", monospace;
        font-size: 0.75rem;
        font-weight: 500;
        text-align: left;
        cursor: default;
    }

    .cell-ref-input:focus {
        outline: 1px solid var(--wx-color-primary);
        cursor: text;
        color: var(--wx-color-font);
    }

    .cell-ref-input::placeholder {
        color: var(--tonic-text-muted);
    }

    .content-group {
        display: flex;
        align-items: center;
        gap: 0.5rem;
        flex: 1;
        height: 100%;
    }

    .formula-label {
        font-family: "JetBrains Mono", monospace;
        font-size: 0.6875rem;
        font-weight: 500;
        color: var(--tonic-text-muted);
        min-width: fit-content;
        display: flex;
        align-items: center;
    }

    .content-group :global(.top-panel-editor) {
        flex: 1;
        width: 0;
        height: 1.5rem;
        background: transparent;
        border: none;
        overflow: hidden;
    }

    .content-group :global(.top-panel-editor.disabled) {
        background: transparent;
    }

    .content-group :global(.top-panel-editor .editor) {
        padding: 0.1875rem 0.625rem;
    }

    .content-group :global(.top-panel-editor .backdrop) {
        padding: 0.1875rem 0.625rem;
    }
</style>
