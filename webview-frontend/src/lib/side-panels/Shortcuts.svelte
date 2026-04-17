<script lang="ts">
    let { onclose }: { onclose: () => void } = $props();
</script>

<div class="panel">
    <div class="header">
        <div>
            <h2>Keyboard shortcuts</h2>
            <p>All available shortcuts and mouse actions</p>
        </div>
        <div class="actions">
            <button class="panel-close" title="Close" onclick={onclose}>
                <svg
                    viewBox="0 0 12 12"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2"
                    stroke-linecap="round"
                >
                    <line x1="2" y1="2" x2="10" y2="10" /><line
                        x1="10"
                        y1="2"
                        x2="2"
                        y2="10"
                    />
                </svg>
            </button>
        </div>
    </div>

    <div class="shortcut-list">
        <section>
            <h3>Navigation</h3>
            <div class="shortcut">
                <kbd>Arrow keys</kbd><span>Move focus</span>
            </div>
            <div class="shortcut">
                <kbd>Enter</kbd><span>Edit cell / confirm and move down</span>
            </div>
            <div class="shortcut">
                <kbd>Escape</kbd><span>Cancel editing, exit clone mode</span>
            </div>
        </section>

        <section>
            <h3>Selection</h3>
            <div class="shortcut">
                <kbd>Shift + Arrow</kbd><span>Extend selection</span>
            </div>
            <div class="shortcut">
                <kbd>Ctrl + A</kbd><span>Select all cells</span>
            </div>
        </section>

        <section>
            <h3>Editing</h3>
            <div class="shortcut">
                <kbd>Backspace</kbd><span>Start editing (delete last char)</span
                >
            </div>
            <div class="shortcut">
                <kbd>Delete</kbd><span>Clear cell or selection</span>
            </div>
            <div class="shortcut"><kbd>Ctrl + Z</kbd><span>Undo</span></div>
            <div class="shortcut">
                <kbd>Ctrl + Shift + Z</kbd><span>Redo</span>
            </div>
        </section>

        <section>
            <h3>Clipboard</h3>
            <div class="shortcut">
                <kbd>Ctrl + C</kbd><span>Copy selection (formulas)</span>
            </div>
            <div class="shortcut">
                <kbd>Ctrl + Shift + C</kbd><span
                    >Copy selection (values only)</span
                >
            </div>
            <div class="shortcut">
                <kbd>Ctrl + V</kbd><span>Paste from clipboard</span>
            </div>
        </section>

        <section>
            <h3>Clone mode</h3>
            <div class="shortcut">
                <kbd>Ctrl + D</kbd><span>Enter / exit clone mode</span>
            </div>
            <div class="shortcut">
                <kbd>Ctrl + V</kbd><span
                    >Paste cloned formulas into current selection</span
                >
            </div>
        </section>

        <section>
            <h3>Expand mode</h3>
            <div class="shortcut">
                <kbd>Shift + Shift</kbd><span
                    >Toggle expand mode (double-tap Shift)</span
                >
            </div>
        </section>

        <section>
            <h3>Mouse</h3>
            <div class="shortcut">
                <kbd>Ctrl + Click</kbd><span>Add cell to multi-selection</span>
            </div>
            <div class="shortcut">
                <kbd>Shift + Click</kbd><span
                    >Extend selection to clicked cell</span
                >
            </div>
            <div class="shortcut">
                <kbd>Alt + Click</kbd><span>Move focus without editing</span>
            </div>
            <div class="shortcut">
                <kbd>Click in formula</kbd><span
                    >Insert cell reference at cursor</span
                >
            </div>
            <div class="shortcut">
                <kbd>Right-click</kbd><span>Context menu</span>
            </div>
        </section>

        <section>
            <h3>File</h3>
            <div class="shortcut"><kbd>Ctrl + S</kbd><span>Save</span></div>
            <div class="shortcut">
                <kbd>Ctrl + Shift + S</kbd><span>Save As</span>
            </div>
        </section>
    </div>
</div>

<style>
    .panel {
        position: relative;
        width: min(40vw, 460px);
        min-width: 340px;
        height: 100%;
        box-sizing: border-box;
        padding: 16px;
        display: flex;
        flex-direction: column;
        gap: 12px;
        background: var(--wx-background);
        color: var(--wx-color-font);
        border-left: var(--wx-border-medium);
        border-top: var(--wx-border-medium);
    }

    .panel-close {
        display: flex;
        align-items: center;
        justify-content: center;
        width: 22px;
        height: 22px;
        background: none;
        border: none;
        cursor: pointer;
        border-radius: 2px;
        color: var(--tonic-text-dim);
        padding: 0;
        flex-shrink: 0;
        transition:
            color 100ms ease,
            background 100ms ease;
    }

    .panel-close:hover {
        color: var(--wx-color-font);
        background: var(--tonic-btn-hover-bg);
    }

    .panel-close svg {
        width: 10px;
        height: 10px;
    }

    .header {
        display: flex;
        align-items: flex-start;
        justify-content: space-between;
        gap: 14px;
        padding: 0 0 12px;
        border-bottom: var(--wx-border-light);
    }

    .header h2 {
        margin: 0;
        font-size: 13px;
        font-family: "JetBrains Mono", monospace;
        color: var(--wx-color-font);
        letter-spacing: 0.03em;
    }

    .header p {
        margin: 3px 0 0;
        opacity: 0.4;
        font-size: 11px;
    }

    .actions {
        display: flex;
        gap: 8px;
        flex-shrink: 0;
    }

    .shortcut-list {
        flex: 1;
        overflow-y: auto;
        display: flex;
        flex-direction: column;
        gap: 4px;
    }

    section {
        border: var(--wx-border-light);
        border-radius: 2px;
        background: var(--wx-background-hover);
        padding: 10px 14px;
    }

    h3 {
        margin: 0 0 6px;
        font-size: 11px;
        font-family: "JetBrains Mono", monospace;
        text-transform: uppercase;
        letter-spacing: 0.06em;
        color: var(--tonic-text-muted);
    }

    .shortcut {
        display: flex;
        align-items: baseline;
        gap: 12px;
        padding: 3px 0;
        font-size: 12px;
    }

    .shortcut span {
        opacity: 0.55;
        font-size: 11px;
    }

    kbd {
        font-family: "JetBrains Mono", monospace;
        font-size: 11px;
        min-width: 120px;
        flex-shrink: 0;
        color: var(--tonic-text-dim);
    }
</style>
