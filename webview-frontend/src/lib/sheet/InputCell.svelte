<script lang="ts">
    import { getSheetSharedState } from "./shared";

    let {
        disabled = false,
        class: className = "",
        editorInputIsFormula,
        editorInput = $bindable(""),
        editorInputHtml,
        oninput,
        onchange,
        onkeydown,
    }: {
        disabled?: boolean;
        class?: string;
        editorInputIsFormula: boolean;
        editorInput: string;
        editorInputHtml: string;
        oninput?: (ev: Event) => void;
        onchange?: (ev: Event) => void;
        onkeydown?: (ev: KeyboardEvent) => void;
    } = $props();

    const shared = getSheetSharedState();

    let backdropEl: HTMLDivElement | undefined = $state();

    function updateCaretPosition(ev: Event) {
        const input = ev.target as HTMLInputElement;
        shared.caretPosition = input.selectionStart ?? 0;
        if (backdropEl) backdropEl.scrollLeft = input.scrollLeft;
    }
</script>

<div
    class="formula-input {className}"
    class:formula={editorInputIsFormula}
    class:disabled
>
    {#if editorInputIsFormula}
        <div class="backdrop" bind:this={backdropEl} aria-hidden="true">
            {@html editorInputHtml}
        </div>
    {/if}
    <input
        class="editor"
        type="text"
        bind:value={editorInput}
        {disabled}
        oninput={(ev) => {
            updateCaretPosition(ev);
            oninput?.(ev);
        }}
        {onchange}
        {onkeydown}
        onclick={updateCaretPosition}
        onkeyup={updateCaretPosition}
        onselect={updateCaretPosition}
    />
</div>

<style>
    @font-face {
        font-family: "JetBrains Mono";
        src: url("/JetBrainsMono-Regular.ttf") format("truetype");
        font-weight: 400;
        font-style: normal;
    }

    .formula-input {
        position: relative;
        width: 100%;
        height: 100%;
    }

    .backdrop {
        position: absolute;
        inset: 0;
        pointer-events: none;
        white-space: pre;
        overflow: hidden;
        font: inherit;
        color: inherit;
    }

    .formula .backdrop,
    .formula .editor {
        font-family: "JetBrains Mono", monospace;
    }

    .editor {
        position: relative;
        width: 100%;
        height: 100%;
        box-sizing: border-box;
        border: none;
        outline: none;
        padding: inherit;
        margin: inherit;
        font: inherit;
        background: transparent;
        color: inherit;

        overflow-y: hidden;
    }

    .formula .editor {
        color: transparent;
        caret-color: var(--wx-input-font-color, rgba(255, 255, 255, 0.9));
    }

    .backdrop :global(.formula-fn-name) {
        color: #61afef;
    }

    .backdrop :global(.formula-cell-reference) {
        color: color-mix(in srgb, var(--ref-color-bg) 70%, white);
    }
</style>
