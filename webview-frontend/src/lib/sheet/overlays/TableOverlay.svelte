<script lang="ts">
    import { invoke } from "@tauri-apps/api/core";
    import { fade } from "svelte/transition";
    import { Icon } from "@svar-ui/svelte-core";
    import { showError } from "$lib/notice";
    import {
        cellRangeToPixels,
        toTranslate3d,
        type CellRange,
        type SheetObjectsState,
        type PixelRect,
    } from "./Overlays.svelte";
    import type { TableData } from "$lib/sheet/shared";

    let {
        sos,
        table,
    }: {
        sos: SheetObjectsState;
        table: TableData;
    } = $props();

    function handleTitleBlur(e: Event) {
        const el = e.target as HTMLElement;
        const newName = (el.textContent ?? "").trim();
        if (!newName || newName === table.title) {
            el.textContent = table.title;
            return;
        }
        const oldName = table.title;
        invoke("change_table_name", { oldName, newName })
            .then(() => {
                table.title = newName;
            })
            .catch((err) => {
                showError("Rename table failed: " + err);
                el.textContent = table.title;
            });
    }

    function handleTitleKeyDown(e: KeyboardEvent) {
        if (e.key === "Enter") {
            e.preventDefault();
            (e.target as HTMLElement).blur();
        }
    }

    function handleApply() {
        invoke("apply_table_projection", { tableName: table.title }).catch(
            (e) => showError(String(e)),
        );
    }

    function handleCancel() {
        invoke("disable_table_projection", { tableName: table.title }).catch(
            (e) => showError(String(e)),
        );
    }

    let borderRect: PixelRect | null = $state(null);
    let titleRect: PixelRect | null = $state(null);
    let hiddenRect: PixelRect | null = $state(null);

    const TITLE_HEIGHT = 18;

    let fullBounds: CellRange = $derived({
        minR: table.headerBounds.minR,
        maxR: table.bodyBounds.maxR,
        minC: Math.min(table.headerBounds.minC, table.bodyBounds.minC),
        maxC: Math.max(table.headerBounds.maxC, table.bodyBounds.maxC),
    });

    let hiddenBounds: CellRange | null = $derived(
        table.hiddenRowsCount > 0
            ? {
                  minR: table.bodyBounds.maxR - table.hiddenRowsCount + 1,
                  maxR: table.bodyBounds.maxR,
                  minC: table.bodyBounds.minC,
                  maxC: table.bodyBounds.maxC,
              }
            : null,
    );

    export function reposition() {
        const fr = cellRangeToPixels(sos, fullBounds);
        if (fr) {
            borderRect = {
                left: fr.left,
                top: fr.top - TITLE_HEIGHT,
                width: fr.width,
                height: fr.height + TITLE_HEIGHT,
            };
            titleRect = {
                left: fr.left,
                top: fr.top - TITLE_HEIGHT,
                width: fr.width,
                height: TITLE_HEIGHT + 2,
            };
        } else {
            borderRect = null;
            titleRect = null;
        }

        if (hiddenBounds) {
            hiddenRect = cellRangeToPixels(sos, hiddenBounds);
        } else {
            hiddenRect = null;
        }
    }
</script>

{#if titleRect}
    <div
        class="table-title"
        style:transform={toTranslate3d(titleRect)}
        style:width="{titleRect.width}px"
        style:height="{titleRect.height}px"
    >
        <span
            class="title-text"
            contenteditable="true"
            role="textbox"
            tabindex="0"
            style="outline: none"
            onblur={handleTitleBlur}
            onkeydown={handleTitleKeyDown}>{table.title}</span
        >

        {#if table.hasProjection}
            <div class="proj-buttons" transition:fade={{ duration: 150 }}>
                <Icon
                    css="wxi wxi-check proj-btn apply-btn"
                    title="Update table"
                    onclick={handleApply}
                />
                <Icon
                    css="wxi wxi-close proj-btn cancel-btn"
                    title="Cancel table update"
                    onclick={handleCancel}
                />
            </div>
        {/if}
    </div>
{/if}
{#if borderRect}
    <div
        class="table-border"
        style:transform={toTranslate3d(borderRect)}
        style:width="{borderRect.width}px"
        style:height="{borderRect.height}px"
    ></div>
    {#if table.hasProjection}
        <div
            class="table-border has-projection"
            transition:fade={{ duration: 150 }}
            style:transform={toTranslate3d(borderRect)}
            style:width="{borderRect.width}px"
            style:height="{borderRect.height}px"
        ></div>
    {/if}
{/if}
{#if hiddenRect}
    <div
        class="hidden-rows-overlay"
        style:transform={toTranslate3d(hiddenRect)}
        style:width="{hiddenRect.width}px"
        style:height="{hiddenRect.height}px"
    >
        <span class="hidden-rows-text">Hidden {table.hiddenRowsCount} rows</span
        >
    </div>
{/if}

<style>
    .table-border {
        position: absolute;
        border-radius: 1px;
        pointer-events: none;
        /* important to have both border and outline of this size, otherwise cell's border is going to be visible */
        border: 1px solid var(--tonic-table-header-cell-bg);
        outline: 1px solid var(--tonic-table-header-cell-bg);
        box-shadow:
            0 0 0 0 transparent,
            0 0 0 transparent,
            0 0 0 transparent,
            0 0 0 transparent,
            0 0 0 transparent;
    }

    .table-border.has-projection::after {
        content: "";
        position: absolute;
        inset: -2px;
        border-radius: 1px;
        will-change: background-position;
        background:
            linear-gradient(90deg, var(--wx-color-primary) 50%, transparent 0) 0
                0 / 20px 1px repeat-x,
            linear-gradient(90deg, var(--wx-color-primary) 50%, transparent 0) 0
                100% / 20px 1px repeat-x,
            linear-gradient(0deg, var(--wx-color-primary) 50%, transparent 0) 0
                0 / 1px 20px repeat-y,
            linear-gradient(0deg, var(--wx-color-primary) 50%, transparent 0)
                100% 0 / 1px 20px repeat-y;
        animation: table-ants 0.8s linear infinite;
    }

    @keyframes table-ants {
        100% {
            background-position:
                20px 0,
                -20px 100%,
                0 -20px,
                100% 20px;
        }
    }

    .table-title {
        position: absolute;
        color: var(--tonic-table-header-cell-color);
        font-family: "JetBrains Mono", monospace;
        font-size: 11px;
        font-weight: 600;
        letter-spacing: 0.03em;
        display: flex;
        align-items: center;
        justify-content: center;
        pointer-events: auto;
        overflow: hidden;
        white-space: nowrap;
        background: var(--tonic-table-header-cell-bg);
    }

    .title-text {
        text-overflow: ellipsis;
        overflow: hidden;
    }

    .proj-buttons {
        display: flex;
        align-items: center;
        gap: 4px;
        margin-left: 4px;
        flex-shrink: 0;
    }

    :global(.proj-btn) {
        font-size: 10px !important;
        cursor: pointer;
        width: 12px;
        height: 12px;
        display: flex !important;
        align-items: center;
        justify-content: center;
        padding: 0 !important;
        border-radius: 3px;
        transition: background 100ms ease;
    }

    :global(.apply-btn) {
        background: var(--wx-color-success);
        color: white;
    }

    :global(.apply-btn:hover) {
        background: var(--wx-color-success) !important;
        filter: brightness(0.85);
    }

    :global(.cancel-btn) {
        background: var(--wx-color-danger);
        color: white;
    }

    :global(.cancel-btn:hover) {
        background: var(--wx-color-danger) !important;
        filter: brightness(0.85);
    }

    .table-border.has-projection {
        box-shadow:
            0 0 0 1px var(--tonic-cell-tint),
            inset 0 1px 0 var(--tonic-cell-tint),
            inset 0 -1px 0 rgba(0, 0, 0, 0.3),
            0 0 12px rgba(0, 0, 0, 0.5),
            0 0 30px rgba(0, 0, 0, 0.3);
    }

    .hidden-rows-overlay {
        position: absolute;
        background: var(--tonic-highlight-bg);
        pointer-events: none;
        display: flex;
        align-items: flex-start;
        justify-content: center;
        padding-top: 8px;
    }

    .hidden-rows-text {
        color: var(--tonic-text-dim);
        font-family: "JetBrains Mono", monospace;
        font-size: 10px;
        font-weight: 500;
        letter-spacing: 0.05em;
    }
</style>
