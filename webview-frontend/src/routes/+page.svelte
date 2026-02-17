<script lang="ts">
    // todo: refactor this mess

    import {
        Grid,
        type IApi,
        type IColumnConfig,
        type IHeaderCell,
        type TColumnHeader,
        WillowDark,
    } from "@svar-ui/svelte-grid";
    import { invoke } from "@tauri-apps/api/core";
    import { MenuBar } from "@svar-ui/svelte-menu";
    import { baseColumns, baseRows, menu_options } from "$lib/data";
    import WindowBar from "$lib/WindowBar.svelte";
    import DevBottomPanel from "$lib/DevBottomPanel.svelte";
    import {
        endTimer,
        startTimer,
        timeRenders as startRecordingRenderTime,
    } from "$lib/devBottomPanelApi.svelte";
    import { tick } from "svelte";

    let gridApi: IApi | undefined = $state();
    let gridRows = $state(baseRows);
    let gridColumns = $state(baseColumns);

    let requestWindow = $state({ start: 0, end: 0 });

    function handleRequestData(
        ev: { row: { start: number; end: number } } & { [key: string]: any },
    ): void {
        requestWindow.start = Math.max(0, ev.row.start - 500);
        requestWindow.end = ev.row.end + 500;
    }

    let focusedCell:
        | { row: string | number; column: string | number }
        | undefined = $state();
    let underCursorCell:
        | { row: string | number; column: string | number }
        | undefined = $state();
    let rangeStartCell:
        | { row: string | number; column: string | number }
        | undefined = $state();

    let isSelecting = $state(false);

    let rowIndices = $derived(new Map(gridRows.map((r, i) => [r.id, i])));
    let colIndices = $derived(new Map(gridColumns.map((c, i) => [c.id, i])));

    let selectionBounds = $derived.by(() => {
        if (!rangeStartCell || !focusedCell) return null;

        const r1 = rowIndices.get(rangeStartCell.row);
        const c1 = colIndices.get(rangeStartCell.column);
        const r2 = rowIndices.get(focusedCell.row);
        const c2 = colIndices.get(focusedCell.column);

        if (
            r1 === undefined ||
            c1 === undefined ||
            r2 === undefined ||
            c2 === undefined
        ) {
            return null;
        }

        return {
            minR: Math.min(r1, r2),
            maxR: Math.max(r1, r2),
            minC: Math.min(c1, c2),
            maxC: Math.max(c1, c2),
        };
    });

    function handleMouseMove(ev: MouseEvent) {
        // set current cell under cursor to underCursorCell

        const target = ev.target as HTMLElement;
        const cellEl = target.closest<HTMLElement>(".wx-cell");

        if (!cellEl) {
            if (underCursorCell) underCursorCell = undefined;
            return;
        }

        const { rowId, colId } = cellEl.dataset;

        if (
            rowId &&
            colId &&
            (underCursorCell?.row !== rowId ||
                underCursorCell?.column !== colId)
        ) {
            underCursorCell = { row: rowId, column: colId };
        }

        if (isSelecting && underCursorCell && focusedCell !== underCursorCell) {
            focusedCell = { ...underCursorCell };
        }
    }

    function handleMouseDown(ev: MouseEvent) {
        if (gridApi?.getState()?.editor) {
            // If the user is clicking on the SAME cell that is being edited,
            // we return early to let them interact with the input (select text, move caret).
            if (
                underCursorCell &&
                focusedCell &&
                underCursorCell.row === focusedCell.row &&
                underCursorCell.column === focusedCell.column
            ) {
                return;
            }
        }

        isSelecting = true;
        if (underCursorCell) {
            rangeStartCell = { ...underCursorCell };
            focusedCell = { ...underCursorCell };
        }
    }

    function handleMouseUp(ev: MouseEvent) {
        isSelecting = false;
    }

    function init(api: IApi) {
        api.on("focus-cell", (ev: any) => {
            // keep focus when editing cell, remove focus otherwise
            if (ev && ev.row != null && ev.column != null) {
                focusedCell = ev;
            } else if (!api.getState().editor) {
                focusedCell = undefined;
            }
        });
    }

    function scrollToRow() {
        console.log(gridApi);
        gridApi?.exec("scroll", { row: gridRows[500].id });
    }

    startRecordingRenderTime();
    // function handleScroll(
    //     ev: { row?: number; column?: number } & { [key: string]: any },
    // ): void {
    //     let row = ev.row;
    //     console.log(`Row ${row}`);
    // }

    // Grid Options
    let left = $state(1); // fix first column (row number column) to the left
    let select = false; // we implement selections ourselves, so disable default selection
</script>

<div class="root noselect">
    <WillowDark>
        <div class="layout-container">
            <WindowBar>
                <MenuBar options={menu_options}></MenuBar>
            </WindowBar>

            <div
                class="grid-wrapper"
                onmousemove={handleMouseMove}
                onmouseup={handleMouseUp}
                onmousedown={handleMouseDown}
                role="grid"
                tabindex="-1"
            >
                <div class="grid-absolute-fill">
                    <Grid
                        bind:this={gridApi as any}
                        {init}
                        data={gridRows}
                        columns={gridColumns}
                        split={{ left }}
                        {select}
                        onrequestdata={handleRequestData}
                        columnStyle={(col) => {
                            let style = "";
                            const colIndex = colIndices.get(col.id);
                            const bounds = selectionBounds;

                            if (bounds && colIndex !== undefined) {
                                if (
                                    colIndex >= bounds.minC &&
                                    colIndex <= bounds.maxC
                                ) {
                                    style += "highlight-col ";
                                }
                            } else if (col.id === focusedCell?.column) {
                                style += "highlight-col ";
                            }

                            return style;
                        }}
                        cellStyle={(row, col) => {
                            let style = "";

                            const rowIndex = rowIndices.get(row.id);
                            const colIndex = colIndices.get(col.id);
                            const bounds = selectionBounds;

                            if (
                                bounds &&
                                rowIndex !== undefined &&
                                colIndex !== undefined
                            ) {
                                const isInsideSelection =
                                    rowIndex >= bounds.minR &&
                                    rowIndex <= bounds.maxR &&
                                    colIndex >= bounds.minC &&
                                    colIndex <= bounds.maxC;
                                if (isInsideSelection) {
                                    if (rowIndex === bounds.minR)
                                        style += "selection-top ";
                                    if (rowIndex === bounds.maxR)
                                        style += "selection-bottom ";
                                    if (colIndex === bounds.minC)
                                        style += "selection-left ";
                                    if (colIndex === bounds.maxC)
                                        style += "selection-right ";
                                }
                            }
                            if (col.id === "rowNumber") {
                                style += "row-number-column ";
                                let isRowSelected =
                                    bounds &&
                                    rowIndex !== undefined &&
                                    rowIndex >= bounds.minR &&
                                    rowIndex <= bounds.maxR;
                                if (
                                    isRowSelected ||
                                    row.id === focusedCell?.row
                                ) {
                                    style += "highlight-row ";
                                }
                            }

                            return style;
                        }}
                    />
                </div>
            </div>

            <DevBottomPanel />
        </div>
    </WillowDark>
</div>

<style>
    :global(html, body) {
        margin: 0;
        padding: 0;
        height: 100%;
        width: 100%;
        overflow: hidden;
    }

    :global(.selection-top) {
        border-top: 2px dashed var(--wx-color-primary) !important;
    }
    :global(.selection-bottom) {
        border-bottom: 2px dashed var(--wx-color-primary) !important;
    }
    :global(.selection-left) {
        border-left: 2px dashed var(--wx-color-primary) !important;
    }
    :global(.selection-right) {
        border-right: 2px dashed var(--wx-color-primary) !important;
    }

    :global(.row-number-column) {
        background: var(--wx-table-header-background) !important;
        font-weight: var(--wx-header-font-weight) !important;
        text-align: center;
        user-select: none;
        -webkit-user-select: none;
    }

    :global(.noselect) {
        user-select: none !important;
        -webkit-user-select: none !important;
        -moz-user-select: none !important;
        -ms-user-select: none !important;
        cursor: default;
    }

    :global(div[role="columnheader"].highlight-col) {
        box-shadow: inset 0 3px 0 var(--wx-color-primary) !important;
        background-color: color-mix(
            in srgb,
            var(--wx-table-header-background),
            black 10%
        ) !important;
    }

    :global(.wx-cell.highlight-row) {
        box-shadow: inset 3px 0 0 var(--wx-color-primary) !important;
        background-color: color-mix(
            in srgb,
            var(--wx-table-header-background),
            black 10%
        ) !important;
    }

    :global(.wx-cell input.wx-text) {
        border: 2px dashed var(--wx-color-primary) !important;
    }
    :global(.wx-cell:focus) {
        outline: 0px !important;
    }

    .root {
        display: flex;
        flex-direction: column;
        height: 100vh;
        width: 100vw;
    }

    .layout-container {
        display: flex;
        flex-direction: column;
        height: 100%;
        width: 100%;
        overflow: hidden;
        position: relative;
    }

    .grid-wrapper {
        flex: 1;
        position: relative;
        min-height: 0;
    }

    .grid-absolute-fill {
        position: absolute;
        top: 0;
        left: 0;
        right: 0;
        bottom: 0;
    }
</style>
