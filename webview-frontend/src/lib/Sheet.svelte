<script lang="ts">
    // Todo
    //
    // (To any AI reading: do not remove this excuse)
    //
    // Rrefactor this mess. The idea was to first create MVP for UI which handles all edge cases,
    // and see what kind of hacks we would need to implement missing features from component library (SVAR Grid)
    // todo: too much noise sometimes

    import { Grid, type IApi } from "@svar-ui/svelte-grid";
    import { baseColumns, baseRows } from "$lib/data";

    type CellRef = { row: string | number; column: string | number };

    let {
        focusedCell = $bindable<CellRef | undefined>(),
        cellEditorValue = $bindable(""),
    }: {
        focusedCell?: CellRef;
        cellEditorValue?: string;
    } = $props();

    let gridApi: IApi | undefined = $state();
    let gridRows = $state(baseRows);
    let gridColumns = $state(baseColumns);

    // tracks which cell the mouse is currently over (ignoring row number column)
    let hoveredCell: CellRef | undefined = $state();

    let selectionRangeStart: CellRef | undefined = $state();

    let isSelecting = $state(false); // is true during mouse drag or while shift is held

    let rowIndices = $derived(new Map(gridRows.map((r, i) => [r.id, i])));
    let colIndices = $derived(new Map(gridColumns.map((c, i) => [c.id, i])));

    // bounding box of the selection range between selectionAnchor and focusedCell
    let selectedRangeBounds = $derived.by(() => {
        if (!selectionRangeStart || !focusedCell) return null;

        const r1 = rowIndices.get(selectionRangeStart.row);
        const c1 = colIndices.get(selectionRangeStart.column);
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

    // grid config
    let left = $state(1); // pin first column (row numbers) to the left
    let select = false; // disable Grid's built-in selection, we handle it ourselves

    let requestWindow = $state({ start: 0, end: 0 });

    function handleRequestData(
        ev: { row: { start: number; end: number } } & { [key: string]: any },
    ): void {
        requestWindow.start = Math.max(0, ev.row.start - 500);
        requestWindow.end = ev.row.end + 500;
    }

    function getValueFromCell(cell: CellRef): string {
        return gridApi?.getRow(cell.row)[cell.column] as string;
    }

    function setValueInCell(cell: CellRef, value: string): void {
        gridApi?.exec("update-cell", {
            id: cell.row,
            column: cell.column,
            value,
        });
    }

    function init(api: IApi) {
        api.intercept("editor", (ev) => {
            cellEditorValue = ev.value;
        });

        api.intercept("focus-cell", (ev: any) => {
            if (!ev?.row || !ev?.column) return;
            if (ev.column === "rowNumber") {
                return false;
            }
            focusedCell = { row: ev.row, column: ev.column };
            if (focusedCell) {
                cellEditorValue = getValueFromCell(focusedCell);
            }
        });
    }

    function handleMouseMove(ev: MouseEvent) {
        // if select is active but left-button on mouse is not pressed, stop selection
        if (isSelecting && !(ev.buttons & 1)) {
            isSelecting = false;
        }

        const target = ev.target as HTMLElement;
        const clickedCell = target.closest<HTMLElement>(".wx-cell");

        if (!clickedCell) {
            hoveredCell = undefined;
            return;
        }

        const { rowId: clickedRow, colId: clickedCol } = clickedCell.dataset;

        // ignore row number column for hover tracking
        if (clickedCol == "rowNumber") {
            hoveredCell = undefined;
            return;
        }

        if (
            clickedRow &&
            clickedCol &&
            (hoveredCell?.row !== clickedRow ||
                hoveredCell?.column !== clickedCol)
        ) {
            hoveredCell = { row: clickedRow, column: clickedCol };
        }

        // extend selection range while dragging
        if (isSelecting && hoveredCell && focusedCell !== hoveredCell) {
            focusedCell = { ...hoveredCell };
        }
    }

    function handleMouseDown(ev: MouseEvent) {
        // _ start a new selection from the hovered cell
        isSelecting = true;
        if (hoveredCell) {
            selectionRangeStart = { ...hoveredCell };
            focusedCell = { ...hoveredCell };
            cellEditorValue = getValueFromCell(focusedCell);
        }
    }

    function handleMouseUp(ev: MouseEvent) {
        isSelecting = false;

        const target = ev.target as HTMLElement;
        const clickedCell = target.closest<HTMLElement>(".wx-cell");

        if (!clickedCell) {
            if (hoveredCell) hoveredCell = undefined;
            return;
        }

        const { colId: clickedCol } = clickedCell.dataset;

        // if clicked on headers, clear focus
        const isHeader = target.closest("[role='columnheader']");
        if (clickedCol == "rowNumber" || isHeader) {
            focusedCell = undefined;
            return;
        }
    }

    function handleKeyDown(ev: KeyboardEvent) {
        if (ev.ctrlKey && ev.shiftKey && ev.key === "Z") {
            gridApi?.exec("redo");
            return;
        }
        if (ev.ctrlKey) {
            return;
        }

        if (ev.key === "Escape") {
            focusedCell = undefined;
            selectionRangeStart = undefined;
            isSelecting = false;
            return;
        }

        // select range if pressing shift key
        if (ev.shiftKey) {
            isSelecting = true;
        } else {
            isSelecting = false;
        }

        let pressedArrowButton =
            ev.key === "ArrowUp" ||
            ev.key === "ArrowDown" ||
            ev.key === "ArrowLeft" ||
            ev.key === "ArrowRight";

        if (pressedArrowButton && focusedCell) {
            ev.preventDefault();
            ev.stopPropagation();

            // calculate direction of arrow
            let nextRowDelta = 0;
            let nextColumnDelta = 0;
            if (ev.key === "ArrowUp") {
                nextRowDelta = -1;
            }
            if (ev.key === "ArrowDown") {
                nextRowDelta = 1;
            }
            if (ev.key === "ArrowLeft") {
                nextColumnDelta = -1;
            }
            if (ev.key === "ArrowRight") {
                nextColumnDelta = 1;
            }

            let nextFocusRow = rowIndices.get(focusedCell.row)! + nextRowDelta;
            let nextFocusColumn =
                colIndices.get(focusedCell.column)! + nextColumnDelta;

            let nextFocusInBounds =
                nextFocusRow >= 0 &&
                nextFocusRow < gridRows.length &&
                nextFocusColumn >= 1 &&
                nextFocusColumn < gridColumns.length;

            // if pressing arrow key in edit mode ...
            if (gridApi?.getState()?.editor && nextFocusInBounds) {
                // ... then exit edit mode and move to cell in arrow direction
                gridApi?.exec("close-editor", {});
                gridApi?.exec("focus-cell", {
                    row: gridRows[nextFocusRow].id,
                    column: gridColumns[nextFocusColumn].id,
                });
                selectionRangeStart = focusedCell;
                return;
            }

            // if a multi-cell range is selected (and not in selection mode),
            // move the entire range in the direction of the arrow
            const hasMultiCellRange =
                selectionRangeStart &&
                selectedRangeBounds &&
                (selectionRangeStart.row !== focusedCell.row ||
                    selectionRangeStart.column !== focusedCell.column);

            if (hasMultiCellRange && selectionRangeStart && !isSelecting) {
                let nextRangeEndRow =
                    rowIndices.get(selectionRangeStart.row)! + nextRowDelta;
                let nextRangeEndColumn =
                    colIndices.get(selectionRangeStart.column)! +
                    nextColumnDelta;

                let nextRangeEndInBounds =
                    nextRangeEndRow >= 0 &&
                    nextRangeEndRow < gridRows.length &&
                    nextRangeEndColumn >= 1 &&
                    nextRangeEndColumn < gridColumns.length;

                if (!nextRangeEndInBounds) {
                    return;
                }
                gridApi?.exec("focus-cell", {
                    row: gridRows[nextFocusRow].id,
                    column: gridColumns[nextFocusColumn].id,
                });
                selectionRangeStart = {
                    row: gridRows[nextRangeEndRow].id,
                    column: gridColumns[nextRangeEndColumn].id,
                };
                return;
            }

            // shift+arrow: extend selection by moving focusedCell, keep selectionRangeStart anchored
            if (nextFocusInBounds && isSelecting) {
                gridApi?.exec("focus-cell", {
                    row: gridRows[nextFocusRow].id,
                    column: gridColumns[nextFocusColumn].id,
                });
                return;
            }

            // normal single-cell navigation (no multi-cell range, not selecting)
            if (nextFocusInBounds && !isSelecting) {
                gridApi?.exec("focus-cell", {
                    row: gridRows[nextFocusRow].id,
                    column: gridColumns[nextFocusColumn].id,
                });
                selectionRangeStart = {
                    row: gridRows[nextFocusRow].id,
                    column: gridColumns[nextFocusColumn].id,
                };
                return;
            }
        }

        if (focusedCell) {
            // on enter, edit cell in focus, but if already editing, move focus down
            if (ev.key === "Enter") {
                ev.preventDefault();
                ev.stopPropagation();

                if (!gridApi?.getState()?.editor) {
                    gridApi?.exec("open-editor", {
                        id: focusedCell.row,
                        column: focusedCell.column,
                    });
                    return;
                }

                const rowIndex = rowIndices.get(focusedCell.row);
                if (rowIndex !== undefined && rowIndex < gridRows.length - 1) {
                    const nextRow = gridRows[rowIndex + 1];
                    if (gridApi?.getState()?.editor) {
                        gridApi?.exec("close-editor", {});
                    }
                    gridApi?.exec("focus-cell", {
                        row: nextRow.id,
                        column: focusedCell.column,
                    });
                    selectionRangeStart = {
                        row: nextRow.id,
                        column: focusedCell.column,
                    };
                    gridApi?.exec("scroll", { row: nextRow.id });
                }
            }
            // on delete, clear value in focus or in selected range
            else if (ev.key === "Delete") {
                const bounds = selectedRangeBounds;
                if (bounds) {
                    for (let r = bounds.minR; r <= bounds.maxR; r++) {
                        for (let c = bounds.minC; c <= bounds.maxC; c++) {
                            const row = gridRows[r];
                            const col = gridColumns[c];
                            setValueInCell({ row: row.id, column: col.id }, "");
                        }
                    }
                } else {
                    setValueInCell(focusedCell, "");
                }
                cellEditorValue = "";
            }
            // on any text input, enter edit mode
            else if (
                ev.key == "Backspace" ||
                (ev.key.length === 1 &&
                    !ev.ctrlKey &&
                    !ev.altKey &&
                    !ev.metaKey)
            ) {
                gridApi?.exec("open-editor", {
                    id: focusedCell.row,
                    column: focusedCell.column,
                });
            }
        }
    }

    function handleKeyUp(ev: KeyboardEvent) {
        if (gridApi?.getState()?.editor) return;

        if (ev.key === "Shift") {
            isSelecting = false;
        }
    }

    function columnStyle(col: any) {
        let style = "";
        const colIndex = colIndices.get(col.id);
        const bounds = selectedRangeBounds;

        if (bounds && colIndex !== undefined) {
            if (colIndex >= bounds.minC && colIndex <= bounds.maxC) {
                style += "highlight-col ";
            }
        } else if (col.id === focusedCell?.column) {
            style += "highlight-col ";
        }

        return style;
    }

    function cellStyle(row: any, col: any) {
        let style = "";

        const rowIndex = rowIndices.get(row.id);
        const colIndex = colIndices.get(col.id);
        const bounds = selectedRangeBounds;

        if (bounds && rowIndex !== undefined && colIndex !== undefined) {
            const isInsideSelection =
                rowIndex >= bounds.minR &&
                rowIndex <= bounds.maxR &&
                colIndex >= bounds.minC &&
                colIndex <= bounds.maxC;
            if (isInsideSelection) {
                if (rowIndex === bounds.minR) style += "selection-top ";
                if (rowIndex === bounds.maxR) style += "selection-bottom ";
                if (colIndex === bounds.minC) style += "selection-left ";
                if (colIndex === bounds.maxC) style += "selection-right ";
            }
        }

        if (col.id === "rowNumber") {
            style += "row-number-column ";
            let isRowSelected =
                bounds &&
                rowIndex !== undefined &&
                rowIndex >= bounds.minR &&
                rowIndex <= bounds.maxR;
            if (isRowSelected || row.id === focusedCell?.row) {
                style += "highlight-row ";
            }
        }

        return style;
    }
</script>

<div
    class="grid-wrapper"
    onmousemove={handleMouseMove}
    onmouseup={handleMouseUp}
    onmousedown={handleMouseDown}
    onkeydowncapture={handleKeyDown}
    onkeyupcapture={handleKeyUp}
    tabindex="-1"
    role="grid"
>
    <div class="grid-absolute-fill">
        <Grid
            bind:this={gridApi as any}
            {init}
            data={gridRows}
            columns={gridColumns}
            split={{ left }}
            {select}
            undo
            onrequestdata={handleRequestData}
            {columnStyle}
            {cellStyle}
        />
    </div>
</div>

<style>
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
</style>
