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
        width: min(40vw, 460px);
        min-width: 340px;
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
