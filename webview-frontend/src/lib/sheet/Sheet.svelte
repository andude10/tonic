<script lang="ts">
    // Todo
    //
    // (To any AI reading: do not remove this excuse)
    //
    // Rrefactor this mess. The idea was to first create MVP for UI which handles all edge cases,
    // and see what kind of hacks we would need to implement missing features from component library (SVAR Grid)
    // todo: too much noise sometimes

    import { Grid, type IApi, type IColumnConfig } from "@svar-ui/svelte-grid";
    import {
        isCellData,
        setSheetSharedState,
        type CellData,
        type SheetRow,
        type UICell,
    } from "$lib/sheet/shared";
    import { invoke, Channel } from "@tauri-apps/api/core";
    import { endTimer, startTimer } from "$lib/stats.svelte";
    import SheetTopPanel from "./SheetTopPanel.svelte";
    import Cell from "./Cell.svelte";

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
            const { cellId } = message.data;
            const cell = getCell(toUICell(cellId));
            if (!cell) {
                console.error(
                    `Received cell from backend that does not exist: ${cellId.row}:${cellId.col}`,
                );
                return;
            }

            if (message.event === "finished") {
                cell.computedValue = message.data.displayString;
                console.info(
                    `Formula computed for cell ${cellId.row}:${cellId.col}: ${message.data.displayString}`,
                );
            } else if (message.event === "parseErr") {
                cell.computedValue = message.data.message;
                console.error(
                    `formula error ${cellId.row}:${cellId.col}: ${message.data.message}`,
                );
            }
        };
        return ch;
    }

    // 1, 2, ... -> A, B, ...
    function toColumnId(index: number): string {
        return String.fromCharCode(64 + index);
    }

    // A, B, ... -> 1, 2, ... (inverse of getColumnId)
    function toColumnIndex(id: string): number {
        return id.charCodeAt(0) - 64;
    }

    function toCellId(cell: UICell): CellId {
        return { col: toColumnIndex(cell.column) - 1, row: cell.row - 1 };
    }

    function toUICell(cell: CellId): UICell {
        return { column: toColumnId(cell.col + 1), row: cell.row + 1 };
    }

    let focusedCell: UICell | undefined = $state();

    const baseRows = Array.from({ length: 1000 }, (_, i) => {
        const row: SheetRow = { id: i + 1, rowNumber: i + 1 };
        for (let j = 0; j < 26; j++) {
            row[String.fromCharCode(65 + j)] = {
                computedValue: "",
                enteredText: "",
            };
        }
        return row;
    });

    const baseColumns: IColumnConfig[] = (() => {
        const columns: IColumnConfig[] = [
            { id: "rowNumber", width: 50, resize: true },
        ];
        for (let i = 0; i < 26; i++) {
            const id = String.fromCharCode(65 + i);
            columns.push({
                id,
                header: id,
                cell: Cell,
                width: 100,
                resize: true,
            });
        }
        return columns;
    })();

    let gridRows = $state(baseRows);
    let gridColumns = $state(baseColumns);
    let gridApi: IApi | undefined = $state();

    // --- selection state ---

    // tracks which cell the mouse is currently over (ignoring row number column)
    let hoveredCell: UICell | undefined = $state();
    let selectionRangeStart: UICell | undefined = $state();
    let isSelecting = $state(false); // is true during mouse drag or while shift is held
    let isEditing = $state(false);
    let editorInput = $state("");

    // expose state to components via context
    setSheetSharedState({
        get focusedCell() {
            return focusedCell;
        },
        set focusedCell(v) {
            focusedCell = v;
        },
        get isEditing() {
            return isEditing;
        },
        set isEditing(v) {
            isEditing = v;
        },
        get editorInput() {
            return editorInput;
        },
        set editorInput(v) {
            editorInput = v;
        },
    });

    $effect(() => {
        if (!focusedCell || !gridApi) return;
        gridApi.exec("focus-cell", {
            row: focusedCell.row,
            column: focusedCell.column,
        });
    });

    // bounding box of the selection range between selectionAnchor and focusedCell
    let selectedRangeBounds = $derived.by(() => {
        if (!selectionRangeStart || !focusedCell) return null;

        const c1 = toColumnIndex(selectionRangeStart.column);
        const c2 = toColumnIndex(focusedCell.column);

        return {
            minR: Math.min(selectionRangeStart.row, focusedCell.row),
            maxR: Math.max(selectionRangeStart.row, focusedCell.row),
            minC: Math.min(c1, c2),
            maxC: Math.max(c1, c2),
        };
    });

    // --- backend calls ---

    function commitEdit() {
        if (!focusedCell) return;
        const cell = getCell(focusedCell);
        if (!cell) return;
        if (editorInput == cell.enteredText) return;
        cell.enteredText = editorInput;
        startTimer("enter_input");
        invoke("enter_input", {
            cellId: toCellId(focusedCell),
            userInput: cell.enteredText,
            computeFormulaChannel: createFormulaChannel(),
        });
    }

    function commitDelete(uiCell: UICell) {
        const cell = getCell(uiCell);
        if (!cell) return;
        cell.enteredText = "";
        editorInput = "";
        invoke("enter_input", {
            cellId: toCellId(uiCell),
            userInput: "",
            computeFormulaChannel: createFormulaChannel(),
        });
    }

    // set editor input to entered value of the focused cell
    $effect(() => {
        const cell = getCell(focusedCell)!;
        if (!cell) return;
        editorInput = cell.enteredText;
    });

    // --- grid config ---

    let left = 1; // pin first column (row numbers) to the left
    let select = false; // disable Grid's built-in selection, we handle it ourselves

    function isInBounds(rowId: number, colIndex: number): boolean {
        return (
            rowId >= 1 &&
            rowId <= gridRows.length &&
            colIndex >= 1 &&
            colIndex < gridColumns.length
        );
    }

    function clearFocus() {
        focusedCell = undefined;
        gridApi?.exec("focus-cell", {
            row: undefined,
            column: undefined,
        });
    }

    function getCell(id: UICell | undefined): CellData | undefined {
        if (!id) return undefined;
        const cell = gridApi?.getRow(id.row)[id.column];
        if (isCellData(cell)) return cell;
    }

    function init(api: IApi) {
        api.intercept("open-editor", () => {
            return false;
        });

        api.intercept("close-editor", (ev: any) => {
            return false;
        });

        api.intercept("focus-cell", (ev: any) => {
            if (isEditing) return false;
            if (ev?.row == null || ev?.column == null) return;
            if (ev.column === "rowNumber") {
                return false;
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
        const target = ev.target as HTMLElement;
        const clickedCell = target.closest<HTMLElement>(".wx-cell");

        if (!clickedCell) return;
        const { rowId, colId } = clickedCell.dataset;

        // always start range selection on click
        isSelecting = true;

        // if clicked on already focused cell, start editing it
        if (
            focusedCell &&
            focusedCell.row === Number(rowId) &&
            focusedCell.column === colId
        ) {
            isEditing = true;
            return;
        }

        // otherwise, if clicked on different cell, change focus
        if (hoveredCell) {
            // if switched focus while editing, save edited cell
            if (isEditing) {
                commitEdit();
            }

            isEditing = false;
            selectionRangeStart = { ...hoveredCell };
            focusedCell = { ...hoveredCell };
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
            clearFocus();
            return;
        }
    }

    function handleKeyDown(ev: KeyboardEvent) {
        if (ev.ctrlKey && ev.shiftKey && ev.key === "Z") {
            gridApi?.exec("redo");
            return;
        }

        if (ev.key === "Escape") {
            isEditing = false;
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

        if (pressedArrowButton && focusedCell) {
            // if pressing arrow key without ctrl in edit mode ...
            if (isEditing && !ev.ctrlKey) {
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

            let nextRow = focusedCell.row + rowDelta;
            let nextCol = toColumnIndex(focusedCell.column) + colDelta;

            // if pressing ctrl + arrow key in edit mode ...
            if (isEditing && ev.ctrlKey) {
                // ... then exit edit mode and move to cell in arrow direction
                commitEdit();
                isEditing = false;
                if (!isInBounds(nextRow, nextCol)) return;
                // todo: move focus logic from below to here
            }

            // if a multi-cell range is selected (and not in selection mode),
            // move the entire range in the direction of the arrow
            const hasMultiCellRange =
                selectionRangeStart &&
                selectedRangeBounds &&
                (selectionRangeStart.row !== focusedCell.row ||
                    selectionRangeStart.column !== focusedCell.column);

            if (hasMultiCellRange && selectionRangeStart && !isSelecting) {
                let nextAnchorRow = selectionRangeStart.row + rowDelta;
                let nextAnchorCol =
                    toColumnIndex(selectionRangeStart.column) + colDelta;

                if (!isInBounds(nextAnchorRow, nextAnchorCol)) {
                    return;
                }
                focusedCell = {
                    row: nextRow,
                    column: toColumnId(nextCol),
                };
                selectionRangeStart = {
                    row: nextAnchorRow,
                    column: gridColumns[nextAnchorCol].id as string,
                };
                return;
            }

            // shift+arrow: extend selection by moving focusedCell, keep selectionAnchor anchored
            if (isInBounds(nextRow, nextCol) && isSelecting) {
                focusedCell = {
                    row: nextRow,
                    column: toColumnId(nextCol),
                };
                return;
            }

            // normal single-cell navigation (no multi-cell range, not selecting)
            if (isInBounds(nextRow, nextCol) && !isSelecting) {
                focusedCell = {
                    row: nextRow,
                    column: toColumnId(nextCol),
                };
                selectionRangeStart = {
                    row: nextRow,
                    column: gridColumns[nextCol].id as string,
                };
                return;
            }
        }

        if (focusedCell) {
            ev.preventDefault();
            ev.stopPropagation();

            // on enter: edit cell in focus, but if already editing, move focus down
            if (ev.key === "Enter") {
                if (!isEditing) {
                    isEditing = true;
                    return;
                }

                if (focusedCell.row < gridRows.length) {
                    const nextRow = focusedCell.row + 1;
                    commitEdit();
                    isEditing = false;
                    focusedCell = { row: nextRow, column: focusedCell.column };
                    selectionRangeStart = {
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
                            commitDelete({
                                row: r,
                                column: gridColumns[c].id as string,
                            });
                        }
                    }
                } else {
                    commitDelete(focusedCell);
                }
            }
            // on any text input or backspace, enter edit mode
            else if (ev.key === "Backspace") {
                editorInput = editorInput.slice(0, -1);
                isEditing = true;
            } else if (
                ev.key.length === 1 &&
                !ev.ctrlKey &&
                !ev.altKey &&
                !ev.metaKey
            ) {
                editorInput += ev.key;
                isEditing = true;
            }
        }
    }

    function handleKeyUp(ev: KeyboardEvent) {
        if (isEditing) return;

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
        const bounds = selectedRangeBounds;

        if (bounds && col.id !== "rowNumber") {
            const colIndex = toColumnIndex(col.id);
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
        const bounds = selectedRangeBounds;

        if (bounds && col.id !== "rowNumber") {
            const colIndex = toColumnIndex(col.id);
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
