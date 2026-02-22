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
    import { baseColumns, baseRows, type UICell } from "$lib/data";
    import { invoke, Channel } from "@tauri-apps/api/core";
    import { endTimer, startTimer } from "./devBottomPanelApi.svelte";
    import SheetTopPanel from "./SheetTopPanel.svelte";

    // -- backend (tauri) communication setup --

    type CellId = { row: number; col: number };

    // Union type for ComputeFormulaEvent to properly represent either success or error
    type ComputeFormulaEvent =
        | { event: "finished"; data: { cellId: CellId; displayString: string } }
        | { event: "parseErr"; data: { cellId: CellId; message: string } };

    // todo: use channels or events?
    function createFormulaChannel(): Channel<ComputeFormulaEvent> {
        const ch = new Channel<ComputeFormulaEvent>();
        ch.onmessage = (message: ComputeFormulaEvent) => {
            endTimer("enter_input");
            if (message.event === "finished") {
                // todo: extract conversion between frontend and backend rows/cols into functions
                const column = String.fromCharCode(
                    65 + message.data.cellId.col,
                );
                const row = message.data.cellId.row + 1;
                gridApi?.exec("update-cell", {
                    id: row,
                    column,
                    value: message.data.displayString,
                });
            } else if (message.event === "parseErr") {
                console.error(
                    `formula error ${message.data.cellId.row}:${message.data.cellId.col}: ${message.data.message}`,
                );
            }
        };
        return ch;
    }

    // 1, 2, ... -> A, B, ...
    function getColumnId(index: number): string {
        return String.fromCharCode(64 + index);
    }

    const focusedCell: { ref: UICell | undefined } = $state({
        ref: undefined,
    });
    let gridRows = $state(baseRows);
    let gridColumns = $state(baseColumns);
    let gridApi: IApi | undefined = $state();

    // --- selection state ---

    // tracks which cell the mouse is currently over (ignoring row number column)
    let hoveredCell: UICell | undefined = $state();
    let selectionRangeStart: UICell | undefined = $state();
    let isSelecting = $state(false); // is true during mouse drag or while shift is held
    const isEditing: { val: boolean } = $state({ val: false });
    const editorValue: { val: string } = $state({ val: "" });

    // expose state to Cell components via context
    setContext("focusedCell", focusedCell);
    setContext("isEditing", isEditing);
    setContext("editorValue", editorValue);

    $effect(() => {
        if (!focusedCell.ref || !gridApi) return;
        gridApi.exec("focus-cell", {
            row: focusedCell.ref.row,
            column: focusedCell.ref.column,
        });
    });

    // --- derived state ---

    // column's ID is a string (A, B, C, etc), and column's index is a number (1, 2, etc)
    let columnIndexById = $derived(
        new Map(gridColumns.map((c, i) => [c.id, i])),
    );

    // bounding box of the selection range between selectionAnchor and focusedCell
    let selectedRangeBounds = $derived.by(() => {
        if (!selectionRangeStart || !focusedCell.ref) return null;

        const c1 = columnIndexById.get(selectionRangeStart.column);
        const c2 = columnIndexById.get(focusedCell.ref.column);

        if (c1 === undefined || c2 === undefined) return null;

        return {
            minR: Math.min(selectionRangeStart.row, focusedCell.ref.row),
            maxR: Math.max(selectionRangeStart.row, focusedCell.ref.row),
            minC: Math.min(c1, c2),
            maxC: Math.max(c1, c2),
        };
    });

    // --- backend calls ---

    function commitFocusedCell() {
        if (!focusedCell.ref) return;
        const colIndex = columnIndexById.get(focusedCell.ref.column);
        if (colIndex === undefined) return;
        const value = getValueFromCell(focusedCell.ref);
        if (value === "") return;
        startTimer("enter_input");
        invoke("enter_input", {
            cellId: { row: focusedCell.ref.row - 1, col: colIndex - 1 },
            userInput: value,
            computeFormulaChannel: createFormulaChannel(),
        });
    }

    // --- grid config ---

    let left = 1; // pin first column (row numbers) to the left
    let select = false; // disable Grid's built-in selection, we handle it ourselves
    //let requestWindow = $state({ start: 0, end: 0 });

    function isInBounds(rowId: number, colIndex: number): boolean {
        return (
            rowId >= 1 &&
            rowId <= gridRows.length &&
            colIndex >= 1 &&
            colIndex < gridColumns.length
        );
    }

    function clearFocus() {
        focusedCell.ref = undefined;
        gridApi?.exec("focus-cell", {
            row: undefined,
            column: undefined,
        });
    }

    function getValueFromCell(cell: UICell): string {
        return gridApi?.getRow(cell.row)[cell.column] as string;
    }

    function setValueInCell(cell: UICell, value: string): void {
        gridApi?.exec("update-cell", {
            id: cell.row,
            column: cell.column,
            value,
        });
    }

    function init(api: IApi) {
        api.intercept("open-editor", () => {
            return false;
        });

        api.intercept("close-editor", (ev: any) => {
            return false;
        });

        api.intercept("focus-cell", (ev: any) => {
            if (isEditing.val) return false;
            if (ev?.row == null || ev?.column == null) return;
            if (ev.column === "rowNumber") {
                return false;
            }

            if (isEditing) {
                commitFocusedCell();
            }
        });
    }

    // function handleRequestData(
    //     ev: { row: { start: number; end: number } } & { [key: string]: any },
    // ): void {
    //     requestWindow.start = Math.max(0, ev.row.start - 500);
    //     requestWindow.end = ev.row.end + 500;
    // }

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
        if (isSelecting && hoveredCell && focusedCell.ref !== hoveredCell) {
            focusedCell.ref = { ...hoveredCell };
        }
    }

    // let pendingOpenEditor: CellRef | undefined = undefined;

    function handleMouseDown(ev: MouseEvent) {
        // allow clicking inside the cell editor input without closing it
        // if (isEditing.val) {
        //     return;
        // }

        const target = ev.target as HTMLElement;
        const clickedCell = target.closest<HTMLElement>(".wx-cell");

        if (!clickedCell) return;

        const { rowId, colId } = clickedCell.dataset;

        if (
            focusedCell.ref &&
            focusedCell.ref.row === Number(rowId) &&
            focusedCell.ref.column === colId
        ) {
            isEditing.val = true;
            return;
        }

        // remember if we're clicking the already-focused cell (to open editor on mouseup)
        // pendingOpenEditor =
        //     hoveredCell &&
        //     focusedCell &&
        //     hoveredCell.row === focusedCell.row &&
        //     hoveredCell.column === focusedCell.column &&
        //     !isEditing
        //         ? { ...focusedCell }
        //         : undefined;

        // start a new selection from the hovered cell

        isSelecting = true;
        if (hoveredCell) {
            isEditing.val = false;
            // if (isEditing) {
            //     commitFocusedCell();
            //     closeEditor();
            // }
            selectionRangeStart = { ...hoveredCell };
            focusedCell.ref = { ...hoveredCell };
        }
    }

    function handleMouseUp(ev: MouseEvent) {
        isSelecting = false;

        const target = ev.target as HTMLElement;
        const clickedCell = target.closest<HTMLElement>(".wx-cell");

        if (!clickedCell) {
            if (hoveredCell) hoveredCell = undefined;
            // pendingOpenEditor = undefined;
            return;
        }

        const { colId } = clickedCell.dataset;

        // if clicked on headers, clear focus
        const isHeader = target.closest("[role='columnheader']");
        if (colId == "rowNumber" || isHeader) {
            clearFocus();
            // pendingOpenEditor = undefined;
            return;
        }

        // // open editor if this was a click (not drag) on the already-focused cell
        // if (
        //     pendingOpenEditor &&
        //     hoveredCell &&
        //     hoveredCell.row === pendingOpenEditor.row &&
        //     hoveredCell.column === pendingOpenEditor.column
        // ) {
        //     openEditor(pendingOpenEditor);
        // }
        // pendingOpenEditor = undefined;
    }

    function handleKeyDown(ev: KeyboardEvent) {
        if (ev.ctrlKey && ev.shiftKey && ev.key === "Z") {
            gridApi?.exec("redo");
            return;
        }

        if (ev.key === "Escape") {
            isEditing.val = false;
            clearFocus();
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

        if (pressedArrowButton && focusedCell.ref) {
            // if pressing arrow key without ctrl in edit mode ...
            if (isEditing.val && !ev.ctrlKey) {
                // .. then use arrow key to navigate inside editor
                // (propagate keyDown event further)
                return;
            }

            ev.preventDefault();
            ev.stopPropagation();

            // calculate direction of arrow
            let rowDelta = 0;
            let colDelta = 0;
            if (ev.key === "ArrowUp") rowDelta = -1;
            if (ev.key === "ArrowDown") rowDelta = 1;
            if (ev.key === "ArrowLeft") colDelta = -1;
            if (ev.key === "ArrowRight") colDelta = 1;

            let nextRow = focusedCell.ref.row + rowDelta;
            let nextCol =
                columnIndexById.get(focusedCell.ref.column)! + colDelta;

            // if pressing ctrl + arrow key in edit mode ...
            if (isEditing.val && ev.ctrlKey) {
                // ... then exit edit mode and move to cell in arrow direction
                commitFocusedCell();
                isEditing.val = false;
                if (!isInBounds(nextRow, nextCol)) return;
                // todo: move focus logic from below to here
            }

            // if a multi-cell range is selected (and not in selection mode),
            // move the entire range in the direction of the arrow
            const hasMultiCellRange =
                selectionRangeStart &&
                selectedRangeBounds &&
                (selectionRangeStart.row !== focusedCell.ref.row ||
                    selectionRangeStart.column !== focusedCell.ref.column);

            if (hasMultiCellRange && selectionRangeStart && !isSelecting) {
                let nextAnchorRow = selectionRangeStart.row + rowDelta;
                let nextAnchorCol =
                    columnIndexById.get(selectionRangeStart.column)! + colDelta;

                if (!isInBounds(nextAnchorRow, nextAnchorCol)) {
                    return;
                }
                focusedCell.ref = {
                    row: nextRow,
                    column: getColumnId(nextCol),
                };
                selectionRangeStart = {
                    row: nextAnchorRow,
                    column: gridColumns[nextAnchorCol].id as string,
                };
                return;
            }

            // shift+arrow: extend selection by moving focusedCell, keep selectionAnchor anchored
            if (isInBounds(nextRow, nextCol) && isSelecting) {
                focusedCell.ref = {
                    row: nextRow,
                    column: getColumnId(nextCol),
                };
                return;
            }

            // normal single-cell navigation (no multi-cell range, not selecting)
            if (isInBounds(nextRow, nextCol) && !isSelecting) {
                focusedCell.ref = {
                    row: nextRow,
                    column: getColumnId(nextCol),
                };
                selectionRangeStart = {
                    row: nextRow,
                    column: gridColumns[nextCol].id as string,
                };
                return;
            }
        }

        if (focusedCell.ref) {
            // on enter: edit cell in focus, but if already editing, move focus down
            if (ev.key === "Enter") {
                ev.preventDefault();
                ev.stopPropagation();

                if (!isEditing.val) {
                    isEditing.val = true;
                    return;
                }

                if (focusedCell.ref.row < gridRows.length) {
                    const nextRow = focusedCell.ref.row + 1;
                    commitFocusedCell();
                    isEditing.val = false;
                    focusedCell.ref.row = nextRow;
                    selectionRangeStart = {
                        row: nextRow,
                        column: focusedCell.ref.column,
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
                    setValueInCell(focusedCell.ref, "");
                }
                // cellEditorValue = "";
            }
            // on any text input, enter edit mode
            else if (
                ev.key == "Backspace" ||
                (ev.key.length === 1 &&
                    !ev.ctrlKey &&
                    !ev.altKey &&
                    !ev.metaKey)
            ) {
                if (!isEditing.val) {
                    isEditing.val = true;
                }
            }
        }
    }

    function handleKeyUp(ev: KeyboardEvent) {
        if (isEditing.val) return;

        if (ev.key === "Shift") {
            isSelecting = false;
        }
    }

    $inspect({
        isEditing,
        isSelecting,
        focusedCell,
        selectionRangeStart,
    });

    function columnStyle(col: any) {
        let style = "";
        const colIndex = columnIndexById.get(col.id);
        const bounds = selectedRangeBounds;

        if (bounds && colIndex !== undefined) {
            if (colIndex >= bounds.minC && colIndex <= bounds.maxC) {
                style += "highlight-col ";
            }
        } else if (col.id === focusedCell.ref?.column) {
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
            if (isRowSelected || row.id === focusedCell.ref?.row) {
                style += "highlight-row ";
            }
        }

        return style;
    }
</script>

<SheetTopPanel {gridApi} />

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
