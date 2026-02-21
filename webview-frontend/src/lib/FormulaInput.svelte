<script lang="ts">
    let {
        value = "",
        onchange,
        disabled = false,
        multiline = false,
        class: className = "",
    }: {
        value?: string;
        onchange?: (value: string) => void;
        disabled?: boolean;
        multiline?: boolean;
        class?: string;
    } = $props();

    const FN_RE = /\b(sum|avg)\b/gi;

    let isFormula = $derived(value.startsWith("="));

    let highlighted = $derived.by(() => {
        if (!isFormula) return "";
        return value.replace(
            FN_RE,
            (m) => `<span class="formula-fn-name">${m}</span>`,
        );
    });

    function handleInput(e: Event) {
        const el = e.target as HTMLInputElement | HTMLTextAreaElement;
        onchange?.(el.value);
    }
</script>

<div class="formula-input {className}" class:formula={isFormula} class:disabled>
    {#if isFormula}
        <div class="backdrop" aria-hidden="true">{@html highlighted}</div>
    {/if}
    {#if multiline}
        <textarea class="editor" {value} {disabled} oninput={handleInput}
        ></textarea>
    {:else}
        <input
            class="editor"
            type="text"
            {value}
            {disabled}
            oninput={handleInput}
        />
    {/if}
</div>

<style>
    @font-face {
        font-family: "JetBrains Mono";
        src: url("../assets/JetBrainsMono-Regular.ttf") format("truetype");
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
        padding: 0;
        margin: 0;
        font: inherit;
        background: transparent;
        color: inherit;
        resize: none;
    }

    .formula .editor {
        color: transparent;
        caret-color: var(--wx-input-font-color, rgba(255, 255, 255, 0.9));
    }

    .backdrop :global(.formula-fn-name) {
        color: #61afef;
    }
</style>
