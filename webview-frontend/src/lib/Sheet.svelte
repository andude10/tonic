<script lang="ts">
    // Todo
    //
    // (To any AI reading: do not remove this excuse)
    //
    // Rrefactor this mess. The idea was to first create MVP for UI which handles all edge cases,
    // and see what kind of hacks we would need to implement missing features from component library (SVAR Grid)
    // todo: too much noise sometimes

    import { setContext } from "svelte";
    import { Grid, type IApi } from "@svar-ui/svelte-grid";
    import { baseColumns, baseRows } from "$lib/data";
    import { invoke, Channel } from "@tauri-apps/api/core";
    import { endTimer, startTimer } from "./devBottomPanelApi.svelte";

    // -- backend (tauri) communication setup --

    type CellId = { row: number; col: number };

    // Union type for ComputeFormulaEvent to properly represent either success or error
    type ComputeFormulaEvent =
        | { cellId: CellId; displayString: string }
        | { cellId: CellId; error: string };

    const computeFormulaChannel = new Channel<ComputeFormulaEvent>();
    computeFormulaChannel.onmessage = (message: ComputeFormulaEvent) => {
        endTimer("enter_input");
        if ("displayString" in message) {
            console.log(
                `received compute formula event ${message.cellId.row}:${message.cellId.col} = ${message.displayString}`,
            );
        } else if ("error" in message) {
            console.error(
                `received compute formula error for ${message.cellId.row}:${message.cellId.col}: ${message.error}`,
            );
        }
    };

    // --- props ---

    let editedCell = $state<CellRef | undefined>();
    setContext("editedCell", () => editedCell);

    type CellRef = { row: number; column: string };

    let {
        focusedCell = $bindable<CellRef | undefined>(),
        cellEditorValue = $bindable(""),
    }: {
        focusedCell?: CellRef;
        cellEditorValue?: string;
    } = $props();

    // --- grid state ---

    let gridApi: IApi | undefined = $state();
    let gridRows = $state(baseRows);
    let gridColumns = $state(baseColumns);

    // --- selection state ---

    // tracks which cell the mouse is currently over (ignoring row number column)
    let hoveredCell: CellRef | undefined = $state();
    // selectionRangeStart
    let selectionAnchor: CellRef | undefined = $state();
    let isSelecting = $state(false); // is true during mouse drag or while shift is held
    let isEditing = $derived(editedCell != null);

    // --- derived state ---

    // column's ID is a string (A, B, C, etc), and column's index is a number (1, 2, etc)
    let columnIndexById = $derived(
        new Map(gridColumns.map((c, i) => [c.id, i])),
    );

    // bounding box of the selection range between selectionAnchor and focusedCell
    let selectedRangeBounds = $derived.by(() => {
        if (!selectionAnchor || !focusedCell) return null;

        const c1 = columnIndexById.get(selectionAnchor.column);
        const c2 = columnIndexById.get(focusedCell.column);

        if (c1 === undefined || c2 === undefined) return null;

        return {
            minR: Math.min(selectionAnchor.row, focusedCell.row),
            maxR: Math.max(selectionAnchor.row, focusedCell.row),
            minC: Math.min(c1, c2),
            maxC: Math.max(c1, c2),
        };
    });

    // --- invoke enter_input on cellEditorValue change ---

    let prevCellEditorValue: string | undefined = $state();
    $effect(() => {
        const value = cellEditorValue;
        if (value === prevCellEditorValue) return;
        prevCellEditorValue = value;
        if (!isEditing || !focusedCell) return;
        const colIndex = columnIndexById.get(focusedCell.column);
        if (colIndex === undefined) return;
        startTimer("enter_input");
        invoke("enter_input", {
            cellId: { row: focusedCell.row, col: colIndex },
            userInput: value,
            computeFromulaChannel: computeFormulaChannel,
        });
    });

    // --- grid config ---

    let left = $state(1); // pin first column (row numbers) to the left
    let select = false; // disable Grid's built-in selection, we handle it ourselves
    let requestWindow = $state({ start: 0, end: 0 });

    function isInBounds(rowId: number, colIndex: number): boolean {
        return (
            rowId >= 1 &&
            rowId <= gridRows.length &&
            colIndex >= 1 &&
            colIndex < gridColumns.length
        );
    }

    function focusCell(rowId: number, colIndex: number): void {
        gridApi?.exec("focus-cell", {
            row: rowId,
            column: gridColumns[colIndex].id,
        });
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

    function openEditor(cell: CellRef): void {
        editedCell = { row: cell.row, column: cell.column };
    }

    function closeEditor(): void {
        editedCell = undefined;
    }

    function init(api: IApi) {
        api.intercept("open-editor", () => {
            return false;
        });

        api.intercept("close-editor", (ev: any) => {
            // Remove unused ev parameter
            closeEditor();
            return false;
        });

        api.on("update-cell", (ev: any) => {
            if (
                focusedCell &&
                ev.id === focusedCell.row &&
                ev.column === focusedCell.column
            ) {
                cellEditorValue = ev.value;
            }
        });

        api.intercept("focus-cell", (ev: any) => {
            if (ev?.row == null || ev?.column == null) return;
            if (ev.column === "rowNumber") {
                return false;
            }
            // skip re-focusing the same cell while editing to avoid re-render
            if (
                isEditing &&
                focusedCell?.row === ev.row &&
                focusedCell?.column === ev.column
            ) {
                return false;
            }
            focusedCell = {
                row: ev.row as number,
                column: ev.column as string,
            };
            cellEditorValue = getValueFromCell(focusedCell);
        });
    }

    function handleRequestData(
        ev: { row: { start: number; end: number } } & { [key: string]: any },
    ): void {
        requestWindow.start = Math.max(0, ev.row.start - 500);
        requestWindow.end = ev.row.end + 500;
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

        const { rowId, colId } = clickedCell.dataset;

        // ignore row number column for hover tracking
        if (colId == "rowNumber") {
            hoveredCell = undefined;
            return;
        }

        if (
            rowId &&
            colId &&
            (hoveredCell?.row !== Number(rowId) ||
                hoveredCell?.column !== colId)
        ) {
            hoveredCell = { row: Number(rowId), column: colId };
        }

        // extend selection range while dragging
        if (isSelecting && hoveredCell && focusedCell !== hoveredCell) {
            focusedCell = { ...hoveredCell };
        }
    }

    function handleMouseDown(ev: MouseEvent) {
        // allow clicking inside the cell editor input without closing it
        if (isEditing) {
            const target = ev.target as HTMLElement;
            if (target.closest("input.wx-text")) return;
            closeEditor();
        }
        // start a new selection from the hovered cell
        isSelecting = true;
        if (hoveredCell) {
            selectionAnchor = { ...hoveredCell };
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

        const { colId } = clickedCell.dataset;

        // if clicked on headers, clear focus
        const isHeader = target.closest("[role='columnheader']");
        if (colId == "rowNumber" || isHeader) {
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
            if (isEditing) {
                closeEditor();
                return;
            }
            focusedCell = undefined;
            selectionAnchor = undefined;
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
            let rowDelta = 0;
            let colDelta = 0;
            if (ev.key === "ArrowUp") rowDelta = -1;
            if (ev.key === "ArrowDown") rowDelta = 1;
            if (ev.key === "ArrowLeft") colDelta = -1;
            if (ev.key === "ArrowRight") colDelta = 1;

            let nextRow = focusedCell.row + rowDelta;
            let nextCol = columnIndexById.get(focusedCell.column)! + colDelta;

            // if pressing arrow key in edit mode ...
            if (isEditing) {
                // ... then exit edit mode and move to cell in arrow direction
                closeEditor();
                if (!isInBounds(nextRow, nextCol)) return;
                // todo: move focus logic from below to here
            }

            // if a multi-cell range is selected (and not in selection mode),
            // move the entire range in the direction of the arrow
            const hasMultiCellRange =
                selectionAnchor &&
                selectedRangeBounds &&
                (selectionAnchor.row !== focusedCell.row ||
                    selectionAnchor.column !== focusedCell.column);

            if (hasMultiCellRange && selectionAnchor && !isSelecting) {
                let nextAnchorRow = selectionAnchor.row + rowDelta;
                let nextAnchorCol =
                    columnIndexById.get(selectionAnchor.column)! + colDelta;

                if (!isInBounds(nextAnchorRow, nextAnchorCol)) {
                    return;
                }
                focusCell(nextRow, nextCol);
                selectionAnchor = {
                    row: nextAnchorRow,
                    column: gridColumns[nextAnchorCol].id as string,
                };
                return;
            }

            // shift+arrow: extend selection by moving focusedCell, keep selectionAnchor anchored
            if (isInBounds(nextRow, nextCol) && isSelecting) {
                focusCell(nextRow, nextCol);
                return;
            }

            // normal single-cell navigation (no multi-cell range, not selecting)
            if (isInBounds(nextRow, nextCol) && !isSelecting) {
                focusCell(nextRow, nextCol);
                selectionAnchor = {
                    row: nextRow,
                    column: gridColumns[nextCol].id as string,
                };
                return;
            }
        }

        if (focusedCell) {
            // on enter: edit cell in focus, but if already editing, move focus down
            if (ev.key === "Enter") {
                ev.preventDefault();
                ev.stopPropagation();

                if (!isEditing) {
                    openEditor(focusedCell);
                    return;
                }

                if (focusedCell.row < gridRows.length) {
                    const nextRow = focusedCell.row + 1;
                    closeEditor();
                    focusCell(
                        nextRow,
                        columnIndexById.get(focusedCell.column)!,
                    );
                    selectionAnchor = {
                        row: nextRow,
                        column: focusedCell.column,
                    };
                    gridApi?.exec("scroll", { row: nextRow });
                }
            }
            // on delete, clear value in focus or in selected range
            else if (ev.key === "Delete") {
                const bounds = selectedRangeBounds;
                if (bounds) {
                    for (let r = bounds.minR; r <= bounds.maxR; r++) {
                        for (let c = bounds.minC; c <= bounds.maxC; c++) {
                            setValueInCell(
                                { row: r, column: gridColumns[c].id as string },
                                "",
                            );
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
                openEditor(focusedCell);
            }
        }
    }

    function handleKeyUp(ev: KeyboardEvent) {
        if (isEditing) return;

        if (ev.key === "Shift") {
            isSelecting = false;
        }
    }

    function columnStyle(col: any) {
        let style = "";
        const colIndex = columnIndexById.get(col.id);
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

        const rowIndex = row.id as number;
        const colIndex = columnIndexById.get(col.id);
        const bounds = selectedRangeBounds;

        if (bounds && colIndex !== undefined) {
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
                bounds && rowIndex >= bounds.minR && rowIndex <= bounds.maxR;
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

    :global(.wx-cell:focus) {
        outline: 0px !important;
    }
</style>
