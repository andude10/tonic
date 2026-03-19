<script lang="ts">
    import InputCell from "./InputCell.svelte";
    import { getSheetSharedState, type UICell } from "$lib/sheet/shared";

    const shared = getSheetSharedState();

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
                readonly
                placeholder="—"
            />
        </div>

        <div class="separator">|</div>

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
    }

    .cell-ref-input {
        width: 4.5rem;
        height: 1.5rem;
        padding-left: 0.8rem;
        padding-right: 0.8rem;
        border: none;
        background: transparent;
        color: var(--wx-input-font-color, rgba(255, 255, 255, 0.9));
        font-family: "JetBrains Mono", monospace;
        font-size: 0.75rem;
        font-weight: 500;
        text-align: left;
        cursor: default;
    }

    .cell-ref-input:focus {
        outline: none;
    }

    .cell-ref-input::placeholder {
        color: var(--wx-input-placeholder-color, #9fa1ae);
    }

    .separator {
        color: var(--wx-border-light, #384047);
        font-size: 1rem;
        line-height: 1;
        opacity: 0.5;
    }

    .content-group {
        display: flex;
        align-items: center;
        gap: 0.5rem;
        flex: 1;
        height: 100%;
    }

    .formula-label {
        font-family: var(--wx-font-family, sans-serif);
        font-size: 0.8125rem;
        font-weight: 500;
        color: var(--wx-color-font-alt, #9fa1ae);
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
