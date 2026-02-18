<script lang="ts">
    import { TextArea } from "@svar-ui/svelte-core";

    let {
        focusedCell = $bindable(),
        cellEditorValue = $bindable(""),
    }: {
        focusedCell?: { row: string | number; column: string | number };
        cellEditorValue?: string;
    } = $props();
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

        <div class="separator"></div>

        <div class="content-group">
            <div class="formula-label">ƒx</div>
            <TextArea
                value={cellEditorValue}
                disabled={!focusedCell}
                onchange={(ev) => (cellEditorValue = ev.value)}
            />
        </div>
    </div>
</div>

<style>
    @font-face {
        font-family: "JetBrains Mono";
        src: url("../assets/JetBrainsMono-Regular.ttf") format("truetype");
        font-weight: 400;
        font-style: normal;
    }

    .panel {
        display: flex;
        align-items: center;
        height: 38px;
        padding: 0 10px;
        border-bottom: var(--wx-border, 1px solid #384047);
        user-select: none;
    }

    .panel-content {
        display: flex;
        align-items: center;
        gap: 10px;
        width: 100%;
        height: 100%;
    }

    .cell-ref-group {
        display: flex;
        align-items: center;
    }

    .cell-ref-input {
        width: 72px;
        height: 26px;
        padding: 2px 6px;
        border: var(--wx-input-border, 1px solid #384047);
        border-radius: var(--wx-input-border-radius, 3px);
        background: var(--wx-input-background, #2a2b2d);
        color: var(--wx-input-font-color, rgba(255, 255, 255, 0.9));
        font-family: "JetBrains Mono", monospace;
        font-size: 12px;
        font-weight: 500;
        text-align: center;
        cursor: default;
    }

    .cell-ref-input:focus {
        outline: none;
        border: var(--wx-input-border-focus, 1px solid #7a67eb);
    }

    .cell-ref-input::placeholder {
        color: var(--wx-input-placeholder-color, #9fa1ae);
    }

    .separator {
        width: 1px;
        height: 20px;
        background: var(--wx-border-light, #384047);
        opacity: 0.5;
    }

    .content-group {
        display: flex;
        align-items: center;
        gap: 8px;
        flex: 1;
        height: 100%;
    }

    .formula-label {
        font-family: var(--wx-font-family, sans-serif);
        font-size: 13px;
        font-weight: 500;
        color: var(--wx-color-font-alt, #9fa1ae);
        min-width: fit-content;
        display: flex;
        align-items: center;
    }

    .content-group :global(.wx-textarea) {
        resize: none;
        flex: 1;
        width: 0;
        min-height: 0;
        height: 26px;
        padding: 3px 10px;
        font-family: "JetBrains Mono", monospace;
        font-size: 13px;
    }

    .content-group :global(.wx-textarea[disabled]) {
        background: color-mix(in srgb, var(--wx-input-background), black 10%);
        color: var(--wx-color-font-alt, #9fa1ae);
        cursor: default;
    }
</style>
