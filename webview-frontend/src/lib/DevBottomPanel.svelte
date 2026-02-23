<script lang="ts">
    import { devPanel, resetTimer } from "./stats.svelte";
</script>

<div class="panel">
    {#if Object.keys(devPanel.stats).length === 0}
        <span class="empty">No stats recorded</span>
    {/if}

    {#each Object.entries(devPanel.stats) as [name, time]}
        <div class="stat-group">
            <div class="header">
                <strong>{name}</strong>
                <button
                    onclick={() => resetTimer(name)}
                    aria-label="Reset"
                    title="Reset stats"
                >
                    <svg
                        viewBox="0 0 24 24"
                        fill="none"
                        stroke="currentColor"
                        stroke-width="3"
                        stroke-linecap="round"
                        stroke-linejoin="round"
                    >
                        <path
                            d="M21 12a9 9 0 0 0-9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"
                        />
                        <path d="M3 3v5h5" />
                    </svg>
                </button>
            </div>

            <div class="values">
                <span>{time.toFixed(2)}<small>ms</small></span>
                <span class="high">
                    H: {devPanel.highestTime[name]?.toFixed(2)}
                </span>
            </div>
        </div>
    {/each}
</div>

<style>
    .panel {
        display: flex;
        flex-wrap: wrap;
        gap: 8px;
        padding: 8px;
        font-family: monospace;
        user-select: none;
    }
    .stat-group {
        display: flex;
        flex-direction: column;
        border-radius: 4px;
        padding: 4px 8px;
        min-width: 110px;
    }
    .header {
        display: flex;
        justify-content: space-between;
        align-items: center;
        margin-bottom: 4px;
    }
    button {
        background: none;
        border: none;
        cursor: pointer;
        color: #999;
        padding: 2px;
        display: flex;
        align-items: center;
        opacity: 0.6;
        transition: all 0.2s;
    }
    button:hover {
        color: #f44;
        opacity: 1;
        transform: rotate(-30deg);
    }
    button svg {
        width: 10px;
        height: 10px;
    }
    .values {
        display: flex;
        justify-content: space-between;
        gap: 10px;
        font-size: 13px;
    }
    .high {
        color: #888;
        font-size: 0.85em;
        align-self: center;
    }
    small {
        font-size: 0.7em;
        opacity: 0.6;
    }
    .empty {
        color: #999;
        font-style: italic;
        padding: 4px;
    }
</style>
