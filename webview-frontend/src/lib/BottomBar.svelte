<script lang="ts">
    import { devPanel } from "$lib/stats.svelte";

    let {
        activePanel,
        ontoggle,
    }: {
        activePanel: string | null;
        ontoggle: (id: string) => void;
    } = $props();

    // throttle fps display to 2/s; devPanel.fps updates every rAF (60/s), which
    // would re-render this component every frame unnecessarily
    let displayFps = $state(0);
    let displayGetDisplayCellsCalls = $state(0);
    $effect(() => {
        const id = setInterval(() => {
            displayFps = devPanel.fps;
            displayGetDisplayCellsCalls =
                devPanel.counts.get_display_cells ?? 0;
        }, 500);
        return () => clearInterval(id);
    });
</script>

<div class="bottom-bar">
    <span class="fps">
        {displayFps} fps | get_display_cells: {displayGetDisplayCellsCalls}
    </span>
    <span class="spacer"></span>
    <button
        class="panel-btn"
        class:active={activePanel === "graph"}
        title="Dependency graph"
        onclick={() => ontoggle("graph")}
    >
        <svg
            viewBox="0 0 32 32"
            xmlns="http://www.w3.org/2000/svg"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
            stroke-miterlimit="10"
        >
            <ellipse cx="16" cy="16" rx="7" ry="8" />
            <path d="M13,8c0-1.7,1.3-3,3-3s3,1.3,3,3" />
            <polyline points="16,8 16,18 12,22" />
            <line x1="16" y1="18" x2="20" y2="22" />
            <path d="M7,5v0.4c0,2.3,1.1,4.4,3,5.6l0,0" />
            <path d="M25,5v0.4c0,2.3-1.1,4.4-3,5.6l0,0" />
            <path d="M7,27v-0.4c0-2.3,1.1-4.4,3-5.6l0,0" />
            <path d="M25,27v-0.4c0-2.3-1.1-4.4-3-5.6l0,0" />
            <line x1="3" y1="16" x2="9" y2="16" />
            <line x1="23" y1="16" x2="29" y2="16" />
        </svg>
    </button>
    <button
        class="panel-btn"
        class:active={activePanel === "extensions"}
        title="Extensions"
        onclick={() => ontoggle("extensions")}
    >
        <svg viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg">
            <path
                fill="currentColor"
                d="M11.5 0C10.119 0 9 1.119 9 2.5V4H5C3.895 4 3 4.895 3 6v3c0 .552.448 1 1 1h.357C5.665 10 6.855 10.941 6.986 12.242 7.136 13.739 5.966 15 4.5 15H4c-.552 0-1 .448-1 1v3c0 1.105.895 2 2 2h3c.552 0 1-.448 1-1v-.357C9 18.335 9.941 17.145 11.242 17.014 12.739 16.864 14 18.034 14 19.5V20c0 .552.448 1 1 1h3c1.105 0 2-.895 2-2v-4h1.5C22.881 15 24 13.881 24 12.5 24 11.119 22.881 10 21.5 10H20V6c0-1.105-.895-2-2-2h-4V2.5C14 1.119 12.881 0 11.5 0z"
            />
        </svg>
    </button>
    <button
        class="panel-btn"
        class:active={activePanel === "shortcuts"}
        title="Keyboard shortcuts"
        onclick={() => ontoggle("shortcuts")}
    >
        <svg
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
            stroke-linejoin="round"
            xmlns="http://www.w3.org/2000/svg"
        >
            <rect x="2" y="6" width="20" height="13" rx="2" />
            <line x1="6" y1="10" x2="6" y2="10" stroke-width="3" />
            <line x1="10" y1="10" x2="10" y2="10" stroke-width="3" />
            <line x1="14" y1="10" x2="14" y2="10" stroke-width="3" />
            <line x1="18" y1="10" x2="18" y2="10" stroke-width="3" />
            <line x1="6" y1="14" x2="6" y2="14" stroke-width="3" />
            <line x1="18" y1="14" x2="18" y2="14" stroke-width="3" />
            <line x1="10" y1="14" x2="14" y2="14" stroke-width="2" />
        </svg>
    </button>
</div>

<style>
    .bottom-bar {
        display: flex;
        align-items: center;
        gap: 2px;
        padding: 0 6px;
        height: 28px;
        flex-shrink: 0;
        border-top: var(--wx-border);
        background: var(--wx-background);
        font-family: "JetBrains Mono", monospace;
    }

    .spacer {
        flex: 1;
    }

    .panel-btn {
        display: flex;
        align-items: center;
        justify-content: center;
        width: 26px;
        height: 22px;
        border: none;
        background: none;
        cursor: pointer;
        border-radius: 2px;
        color: var(--tonic-text-muted);
        padding: 0;
        transition:
            color 100ms ease,
            background 100ms ease;
    }

    .panel-btn:hover {
        color: var(--wx-color-font);
        background: var(--tonic-btn-hover-bg);
    }

    .panel-btn.active {
        color: var(--wx-color-primary);
        background: var(--wx-color-primary-selected);
    }

    .panel-btn svg {
        width: 15px;
        height: 15px;
    }

    .fps {
        color: var(--tonic-text-muted);
        font-size: 10px;
        letter-spacing: 0.04em;
        padding: 0 2px;
    }
</style>
