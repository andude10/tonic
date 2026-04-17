<script lang="ts">
    import { invoke } from "@tauri-apps/api/core";
    import { open } from "@tauri-apps/plugin-dialog";
    import { onMount } from "svelte";
    import { loadExtension, unloadExtension } from "$lib/extensions/dispatcher";

    let { onclose }: { onclose: () => void } = $props();

    let scripts: string[] = $state([]);
    let isLoading = $state(true);
    let errorMessage = $state("");

    async function refresh() {
        isLoading = true;
        errorMessage = "";
        try {
            scripts = await invoke<string[]>("list_scripts");
        } catch (e) {
            errorMessage = String(e);
        } finally {
            isLoading = false;
        }
    }

    async function addScript() {
        const path = await open({
            multiple: false,
            directory: false,
            filters: [{ name: "JavaScript", extensions: ["js"] }],
        });
        if (!path) return;

        try {
            // rust reads the file and stores it, returns the content
            const content = await invoke<string>("add_script", {
                filePath: path,
            });
            const fileName = path.split(/[/\\]/).pop() ?? "script.js";
            loadExtension(fileName, content);
            await refresh();
        } catch (e) {
            errorMessage = String(e);
        }
    }

    async function removeScript(name: string) {
        try {
            await invoke("unregister_functions_by_file", { fileName: name });
            unloadExtension(name);
            await invoke("remove_script", { name });
            await refresh();
        } catch (e) {
            errorMessage = String(e);
        }
    }

    onMount(() => {
        refresh();
    });
</script>

<div class="panel">
    <div class="header">
        <div>
            <h2>Extensions</h2>
            <p>JavaScript files attached to this spreadsheet</p>
        </div>
        <div class="actions">
            <button type="button" onclick={addScript}>Add file</button>
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

    {#if isLoading}
        <div class="state">Loading...</div>
    {:else if errorMessage}
        <div class="state error">{errorMessage}</div>
    {:else if scripts.length === 0}
        <div class="state">No extension files attached</div>
    {:else}
        <div class="script-list">
            {#each scripts as name}
                <div class="script-item">
                    <span class="script-name">{name}</span>
                    <button
                        type="button"
                        class="remove-btn"
                        onclick={() => removeScript(name)}
                    >
                        Remove
                    </button>
                </div>
            {/each}
        </div>
    {/if}
</div>

<style>
    .panel {
        position: relative;
        width: min(40vw, 500px);
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

    .script-list {
        display: flex;
        flex-direction: column;
        gap: 2px;
        overflow-y: auto;
        flex: 1;
    }

    .script-item {
        display: flex;
        align-items: center;
        justify-content: space-between;
        padding: 8px 14px;
        border: var(--wx-border-light);
        border-radius: 2px;
        background: var(--wx-background-hover);
    }

    .script-name {
        font-family: "JetBrains Mono", monospace;
        font-size: 12px;
    }

    .remove-btn {
        font-size: 11px !important;
        padding: 4px 10px !important;
        opacity: 0.5;
    }

    .remove-btn:hover {
        opacity: 1;
        color: #fca5a5 !important;
        border-color: rgba(239, 68, 68, 0.3) !important;
    }

    .state {
        display: flex;
        align-items: center;
        justify-content: center;
        flex: 1;
        padding: 16px;
        border: 1px dashed var(--tonic-popup-border-color);
        border-radius: 2px;
        opacity: 0.6;
        font-family: "JetBrains Mono", monospace;
        font-size: 11px;
    }

    .error {
        color: #fca5a5;
        border-color: rgba(239, 68, 68, 0.3);
        background: rgba(239, 68, 68, 0.08);
        opacity: 1;
    }
</style>
