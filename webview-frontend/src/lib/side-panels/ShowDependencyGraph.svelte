<script module lang="ts">
    import { instance, type Viz } from "@viz-js/viz";

    let vizPromise: Promise<Viz> | null = null;

    function getViz() {
        if (!vizPromise) {
            vizPromise = instance();
        }

        return vizPromise;
    }
</script>

<script lang="ts">
    import { invoke } from "@tauri-apps/api/core";
    import { onMount } from "svelte";

    let { onclose }: { onclose: () => void } = $props();

    let dotSource = $state("");
    let svgMarkup = $state("");
    let isLoading = $state(true);
    let errorMessage = $state("");

    async function loadDependencyGraph() {
        isLoading = true;
        errorMessage = "";

        try {
            dotSource = await invoke<string>("get_dependency_graph_dot");

            // render in the webview so the side area stays self-contained
            const viz = await getViz();
            svgMarkup = viz.renderString(dotSource, {
                format: "svg",
                engine: "dot",
            });
        } catch (error) {
            svgMarkup = "";
            errorMessage = String(error);
        } finally {
            isLoading = false;
        }
    }

    onMount(() => {
        loadDependencyGraph();
    });
</script>

<div class="panel">
    <div class="header">
        <div>
            <h2>Dependency graph</h2>
            <p>current spreadsheet dependency graph rendered from DOT</p>
        </div>

        <div class="actions">
            <button type="button" onclick={loadDependencyGraph}>Refresh</button>
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
        <div class="state">Rendering dependency graph...</div>
    {:else if errorMessage}
        <div class="state error">{errorMessage}</div>
    {:else}
        <div class="graph-frame">
            {@html svgMarkup}
        </div>
    {/if}

    <details class="dot-source">
        <summary>DOT source</summary>
        <pre>{dotSource}</pre>
    </details>
</div>

<style>
    .panel {
        width: min(54vw, 960px);
        min-width: 460px;
    }

    .graph-frame {
        flex: 1;
        min-height: 0;
        overflow: auto;
        border: var(--wx-border-light);
        border-radius: 2px;
        padding: 16px;
        background: var(--wx-background-hover);
    }

    .graph-frame :global(svg) {
        width: max-content;
        min-width: 100%;
        height: auto;
        display: block;
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
        white-space: pre-wrap;
        align-items: flex-start;
    }

    .dot-source {
        border: var(--wx-border-light);
        border-radius: 2px;
        background: var(--wx-background-hover);
        overflow: hidden;
    }

    .dot-source summary {
        padding: 10px 14px;
        background: var(--wx-background-alt);
        cursor: pointer;
        font-family: "JetBrains Mono", monospace;
        font-size: 11px;
        color: var(--tonic-text-dim);
    }

    .dot-source pre {
        margin: 0;
        padding: 12px 14px;
        max-height: 180px;
        overflow: auto;
        font-size: 11px;
        opacity: 0.5;
    }

    @media (max-width: 900px) {
        .panel {
            width: 100vw;
            min-width: 0;
        }

        .header {
            flex-direction: column;
        }
    }
</style>
