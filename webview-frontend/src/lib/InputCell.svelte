<script lang="ts">
    let {
        disabled = false,
        class: className = "",
        value = $bindable(""),
        oninput,
    }: {
        disabled?: boolean;
        class?: string;
        value?: string;
        oninput?: (value: string) => void;
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
</script>

<div class="formula-input {className}" class:formula={isFormula} class:disabled>
    {#if isFormula}
        <div class="backdrop" aria-hidden="true">{@html highlighted}</div>
    {/if}
    <input
        class="editor"
        type="text"
        bind:value
        {disabled}
        oninput={() => oninput?.(value)}
    />
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
        padding: inherit;
        margin: inherit;
        font: inherit;
        background: transparent;
        color: inherit;
    }

    .formula .editor {
        color: transparent;
        caret-color: var(--wx-input-font-color, rgba(255, 255, 255, 0.9));
    }

    .backdrop :global(.formula-fn-name) {
        color: #61afef;
    }
</style>
