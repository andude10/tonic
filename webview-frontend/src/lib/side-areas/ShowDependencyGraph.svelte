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
    import { SideArea } from "@svar-ui/svelte-core";
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

<SideArea oncancel={onclose}>
    <div class="panel">
        <div class="header">
            <div>
                <h2>Dependency graph</h2>
                <p>current spreadsheet dependency graph rendered from DOT</p>
            </div>

            <div class="actions">
                <button type="button" onclick={loadDependencyGraph}
                    >Refresh</button
                >
                <button type="button" class="ghost" onclick={onclose}
                    >Close</button
                >
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
</SideArea>

<style>
    .panel {
        width: min(54vw, 960px);
        min-width: 460px;
        height: 100%;
        box-sizing: border-box;
        padding: 16px;
        display: flex;
        flex-direction: column;
        gap: 12px;
        background: var(--wx-background);
        color: rgba(255, 255, 255, 0.88);
        border-left: var(--wx-border);
    }

    .header {
        display: flex;
        align-items: flex-start;
        justify-content: space-between;
        gap: 14px;
        padding: 12px 14px;
        border: var(--wx-border);
        border-radius: var(--wx-border-radius);
        background: var(--wx-background-alt);
    }

    .header h2 {
        margin: 0;
        font-size: 14px;
    }

    .header p {
        margin: 3px 0 0;
        opacity: 0.5;
    }

    .actions {
        display: flex;
        gap: 8px;
        flex-shrink: 0;
    }

    .graph-frame {
        flex: 1;
        min-height: 0;
        overflow: auto;
        border: var(--wx-border);
        border-radius: var(--wx-border-radius);
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
        border: 1px dashed #3e4042;
        border-radius: var(--wx-border-radius);
        opacity: 0.6;
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
        border: var(--wx-border);
        border-radius: var(--wx-border-radius);
        background: var(--wx-background-hover);
        overflow: hidden;
    }

    .dot-source summary {
        padding: 10px 14px;
        background: var(--wx-background-alt);
        cursor: pointer;
    }

    .dot-source pre {
        margin: 0;
        padding: 12px 14px;
        max-height: 180px;
        overflow: auto;
        font-size: 11px;
        opacity: 0.6;
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
