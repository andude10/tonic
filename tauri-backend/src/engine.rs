use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;
use std::time::Instant;

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use tauri_plugin_log::log::{debug, info};

use std::io;

use crate::file_api;
use crate::parser::{lex_formula, parse_formula, FormulaState};
use crate::storage::grid::{Cell, CellValue, GridCellId};
use crate::storage::types::{
    AbsoluteCellId, AtomType, CellRange, Coordinate, Expr, ExprAtom, Formula, FormulaId,
    ProjectionFilterOption, Reference, SheetId, Sheets, Spreadsheet,
};

/// A single cell mutation: (cell_id, old_value, new_value).
#[derive(Clone)]
pub struct CellUpdate(pub AbsoluteCellId, pub Option<Cell>, pub Option<Cell>);

/// Bounding rectangle of affected cells. Returned by undo/redo.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ChangeBounds {
    pub min_row: u32,
    pub max_row: u32,
    pub min_col: u32,
    pub max_col: u32,
}

enum HistoryLogEntry {
    Update {
        id: u64,
        changes: Vec<CellUpdate>,
        bounds: ChangeBounds,
    },
    InsertRow {
        id: u64,
        changes: Vec<CellUpdate>,
        bounds: ChangeBounds,
    },
    InsertColumn {
        id: u64,
        changes: Vec<CellUpdate>,
        bounds: ChangeBounds,
    },
    RemoveRow {
        id: u64,
        changes: Vec<CellUpdate>,
        bounds: ChangeBounds,
    },
    RemoveColumn {
        id: u64,
        changes: Vec<CellUpdate>,
        bounds: ChangeBounds,
    },
}

impl HistoryLogEntry {
    fn id(&self) -> u64 {
        match self {
            HistoryLogEntry::Update { id, .. }
            | HistoryLogEntry::InsertRow { id, .. }
            | HistoryLogEntry::InsertColumn { id, .. }
            | HistoryLogEntry::RemoveRow { id, .. }
            | HistoryLogEntry::RemoveColumn { id, .. } => *id,
        }
    }

    fn bounds(&self) -> &ChangeBounds {
        match self {
            HistoryLogEntry::Update { bounds, .. }
            | HistoryLogEntry::InsertRow { bounds, .. }
            | HistoryLogEntry::InsertColumn { bounds, .. }
            | HistoryLogEntry::RemoveRow { bounds, .. }
            | HistoryLogEntry::RemoveColumn { bounds, .. } => bounds,
        }
    }

    fn changes(&self) -> &Vec<CellUpdate> {
        match self {
            HistoryLogEntry::Update { changes, .. }
            | HistoryLogEntry::InsertRow { changes, .. }
            | HistoryLogEntry::InsertColumn { changes, .. }
            | HistoryLogEntry::RemoveRow { changes, .. }
            | HistoryLogEntry::RemoveColumn { changes, .. } => changes,
        }
    }
}

/// Tracks undo/redo history.
pub struct History {
    log: Vec<HistoryLogEntry>,
    log_position: usize,
    next_log_id: u64,
    last_saved_log_id: Option<u64>,
}

impl History {
    fn new() -> Self {
        Self {
            log: Vec::new(),
            log_position: 0,
            next_log_id: 1,
            last_saved_log_id: None,
        }
    }
}

pub struct EngineGuard(PhantomData<()>);

#[derive(Debug)]
pub enum EvalError {
    TypeError { expected: AtomType, got: AtomType },
    Error(String),
}

struct DebugInfo {
    eval_number: u64,
}

impl DebugInfo {
    fn new() -> Self {
        Self { eval_number: 0 }
    }
}

pub struct Engine {
    // todo: move spreadsheet out of the Engine?
    pub spreadsheet: Spreadsheet,
    history: History,
    batch: Vec<CellUpdate>,
    debug: DebugInfo,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ColumnOrRowChange {
    Insert,
    Remove,
}

#[derive(Clone, Copy)]
struct ColumnOrRowChangeSpec {
    change: ColumnOrRowChange,
    sheet_id: u32,
    changing_row: bool,
    index: u32,
    shift_start: u32,
    deleted_index: Option<u32>,
}

impl ColumnOrRowChangeSpec {
    fn for_insert(sheet_id: u32, changing_row: bool, index: u32, include_index: bool) -> Self {
        let shift_start = if include_index {
            index
        } else {
            index.saturating_add(1)
        };
        Self {
            change: ColumnOrRowChange::Insert,
            sheet_id,
            changing_row,
            index,
            shift_start,
            deleted_index: None,
        }
    }

    fn for_remove(sheet_id: u32, changing_row: bool, index: u32) -> Self {
        Self {
            change: ColumnOrRowChange::Remove,
            sheet_id,
            changing_row,
            index,
            shift_start: index.saturating_add(1),
            deleted_index: Some(index),
        }
    }

    fn operation_name(&self) -> &'static str {
        if self.change == ColumnOrRowChange::Insert {
            "Insert"
        } else {
            "Remove"
        }
    }
}

// todo: add more comments (explain what happens with dependencies, dependants and the grid)

impl Engine {
    pub fn new() -> Self {
        Self {
            spreadsheet: Spreadsheet::new(),
            history: History::new(),
            batch: Vec::new(),
            debug: DebugInfo::new(),
        }
    }

    pub fn start_batch(&mut self) -> EngineGuard {
        self.batch.clear();
        EngineGuard(PhantomData)
    }

    fn set_cell(&mut self, id: &AbsoluteCellId, new_cell: Option<Cell>) -> Option<Cell> {
        let old_cell = self.spreadsheet.get_cell(id).cloned();
        if old_cell
            .as_ref()
            .and_then(|cell| cell.defined_by_formula)
            .is_some()
        {
            self.spreadsheet.dependency_graph.remove_formula_cell(*id);
        }

        match new_cell.as_ref() {
            Some(cell) => self.spreadsheet.insert_cell(id, cell.clone()),
            None => self.spreadsheet.remove_cell(id),
        }

        if let Some(formula_id) = new_cell.as_ref().and_then(|cell| cell.defined_by_formula) {
            if let Some(formula) = self.spreadsheet.formulas.get(formula_id) {
                self.spreadsheet
                    .dependency_graph
                    .insert_formula_cell(*id, &formula.ast);
            }
        }

        old_cell
    }

    // todo: simplify?

    fn record_cell_change(&mut self, id: AbsoluteCellId, new_cell: Option<Cell>) {
        let old_cell = self.set_cell(&id, new_cell.clone());
        self.batch.push(CellUpdate(id, old_cell, new_cell));
    }

    /// Parse string and insert appropriate cell.
    pub fn parse_and_insert_string(&mut self, _: &EngineGuard, id: AbsoluteCellId, input: &str) {
        if !input.starts_with('=') {
            let new = if let Ok(n) = input.parse::<Decimal>() {
                Cell::number(n)
            } else {
                Cell::text(input.to_string())
            };
            self.record_cell_change(id, Some(new));
            return;
        }

        // start parsing formula
        let formula_text = &input[1..];
        let lex_result = lex_formula(formula_text);
        // report any errors during lexing
        if lex_result.has_errors() {
            self.record_cell_change(id, Some(Cell::error("Lex error".into())));
            return;
        }

        // return if empty
        let Some(tokens) = lex_result.into_output() else {
            return;
        };

        let parser_cell_id = GridCellId {
            row: id.row,
            col: id.col,
        };
        let mut state = FormulaState {
            names: &mut self.spreadsheet.names,
            cell_id: parser_cell_id,
            expr_arena: Vec::new(),
        };
        let (parsed, parse_errs) = parse_formula(&tokens, formula_text.len(), &mut state);
        // report any errors during parsing (syntax, name not found)
        if !parse_errs.is_empty() {
            self.record_cell_change(id, Some(Cell::error("Parse error".into())));
            return;
        }
        let Some((ast, _root_id)) = parsed else {
            return;
        };

        // create and store formula
        let formula = Formula {
            ast,
            formula_string: input.to_string(),
        };
        let formula_id = self.spreadsheet.formulas.insert(formula);

        // insert cell with formula reference and dependencies
        let new = Cell {
            defined_by_formula: Some(formula_id),
            val: CellValue::Error(String::new()),
            pending_dependencies: 0,
        };
        self.record_cell_change(id, Some(new));
    }

    pub fn insert_number(&mut self, _: &EngineGuard, id: AbsoluteCellId, n: Decimal) {
        self.record_cell_change(id, Some(Cell::number(n)));
    }

    pub fn insert_value(&mut self, _: &EngineGuard, id: AbsoluteCellId, val: CellValue) {
        let old = self.spreadsheet.get_cell(&id).cloned();
        self.spreadsheet.set_value(&id, val);
        let new = self.spreadsheet.get_cell(&id).cloned();
        self.batch.push(CellUpdate(id, old, new));
    }

    pub fn insert_shared_formula(
        &mut self,
        _: &EngineGuard,
        id: AbsoluteCellId,
        formula_id: FormulaId,
    ) {
        let new = Cell {
            defined_by_formula: Some(formula_id),
            val: CellValue::Error(String::new()),
            pending_dependencies: 0,
        };
        self.record_cell_change(id, Some(new));
    }

    pub fn delete(&mut self, _: &EngineGuard, id: AbsoluteCellId) {
        self.record_cell_change(id, None);
    }

    /// Recalculate all formulas affected by the given cell changes.
    /// All changes should already be applied to the grid, and the dependency graph should be correct
    /// todo: write comments
    fn post_cell_changes_hook(&mut self, changes: Vec<CellUpdate>) {
        self.debug.eval_number += 1;
        info!("Eval #{}", self.debug.eval_number);
        let dep_time = Instant::now();

        // step 1.
        //
        // for each cell X (that is changed, or is (transitive) dependent of changed cell):
        // set "pending_dependencies" to be the number of cells that need to be calculated before X.

        // for each cell in "current", increase "pending_dependencies" of cell's dependents, and push cell's dependents to next
        // after each pass, swap

        let mut wave: Vec<AbsoluteCellId> = Vec::new();
        let changed_cells: Vec<_> = changes.iter().map(|CellUpdate(id, _, _)| *id).collect();
        {
            let spreadsheet = &mut self.spreadsheet;
            let ready_cells = spreadsheet
                .dependency_graph
                .init_pending_counter_for_new_recalculation(
                    &mut spreadsheet.sheets,
                    &changed_cells,
                );
            wave.extend_from_slice(ready_cells);
        }

        let mut dep_duration = dep_time.elapsed();
        let mut eval_duration = std::time::Duration::ZERO;
        let mut eval_store: Vec<ExprAtom> = Vec::new();

        // step 2.
        //
        // todo

        let mut affected_tables: std::collections::HashSet<u32> = std::collections::HashSet::new();
        for cell_id in &changed_cells {
            if let Some((table_id, _)) = self.spreadsheet.find_table_containing_cell(cell_id) {
                affected_tables.insert(table_id);
            }
        }

        while let Some(cell_id) = wave.pop() {
            let formula_id = self
                .spreadsheet
                .get_cell(&cell_id)
                .and_then(|c| c.defined_by_formula);

            if let Some(formula_id) = formula_id {
                if let Some(formula) = self.spreadsheet.formulas.get(formula_id) {
                    let ast = &formula.ast;
                    let t = Instant::now();
                    let new_value = match eval_formula(
                        ast,
                        &cell_id,
                        &self.spreadsheet.sheets,
                        &mut eval_store,
                    ) {
                        Ok(v) => v,
                        Err(EvalError::Error(msg)) => CellValue::Error(msg),
                        Err(e) => CellValue::Error(format!("{:?}", e)),
                    };
                    eval_duration += t.elapsed();

                    self.spreadsheet.set_value(&cell_id, new_value);
                }
            }

            // Decrement pending_dependencies_count of dependents, add to wave if ready
            let t = Instant::now();
            {
                let spreadsheet = &mut self.spreadsheet;
                let ready_cells = spreadsheet
                    .dependency_graph
                    .decrease_pending_counter(&mut spreadsheet.sheets, cell_id);
                wave.extend_from_slice(ready_cells);
            }

            if let Some((table_id, _)) = self.spreadsheet.find_table_containing_cell(&cell_id) {
                affected_tables.insert(table_id);
            }

            dep_duration += t.elapsed();
        }

        // todo: add normal cycle detection
        let t = Instant::now();
        let unresolved_cells = {
            let spreadsheet = &mut self.spreadsheet;
            spreadsheet
                .dependency_graph
                .collect_unresolved_cells(&spreadsheet.sheets)
                .to_vec()
        };
        for cell_id in unresolved_cells {
            if self
                .spreadsheet
                .get_cell(&cell_id)
                .and_then(|cell| cell.defined_by_formula)
                .is_some()
            {
                self.spreadsheet
                    .set_value(&cell_id, CellValue::Error("Cycle".into()));
            }

            if let Some((table_id, _)) = self.spreadsheet.find_table_containing_cell(&cell_id) {
                affected_tables.insert(table_id);
            }
        }
        dep_duration += t.elapsed();

        let table_time = Instant::now();
        for table_id in affected_tables {
            self.post_table_cells_change_hook(table_id);
        }

        info!("Eval (dependencies & dependants) took: {:?}", dep_duration);
        info!("Eval (running expressions) took: {:?}", eval_duration);
        info!("Eval (updating tables) took: {:?}", table_time.elapsed());
    }

    /// Finalize the batch: push to undo log, truncate redo history.
    pub fn end_batch(&mut self, _: EngineGuard) {
        if self.batch.is_empty() {
            return;
        }

        let changes = std::mem::take(&mut self.batch);
        let bounds = compute_bounds(&changes);
        let eval_changes = changes.clone();

        self.history.log.truncate(self.history.log_position);
        let id = self.history.next_log_id;
        self.history.next_log_id += 1;

        self.history.log.push(HistoryLogEntry::Update {
            id,
            changes,
            bounds,
        });
        self.history.log_position = self.history.log.len();

        self.post_cell_changes_hook(eval_changes);
    }

    fn apply_history_changes(&mut self, changes: &[CellUpdate], use_new: bool) {
        if use_new {
            for CellUpdate(id, _, new) in changes {
                self.set_cell(id, new.clone());
            }
        } else {
            // when undoing, walk backward to revert writes in exact reverse order
            for CellUpdate(id, old, _) in changes.iter().rev() {
                self.set_cell(id, old.clone());
            }
        }
    }

    pub fn undo(&mut self) -> Option<ChangeBounds> {
        if self.history.log_position == 0 {
            return None;
        }
        self.history.log_position -= 1;
        let entry = &self.history.log[self.history.log_position];
        let bounds = entry.bounds().clone();
        let changes = entry.changes().clone();

        self.apply_history_changes(&changes, false);
        self.post_cell_changes_hook(changes);
        Some(bounds)
    }

    pub fn redo(&mut self) -> Option<ChangeBounds> {
        if self.history.log_position >= self.history.log.len() {
            return None;
        }
        let entry = &self.history.log[self.history.log_position];
        let bounds = entry.bounds().clone();
        let changes = entry.changes().clone();
        self.history.log_position += 1;

        self.apply_history_changes(&changes, true);
        self.post_cell_changes_hook(changes);
        Some(bounds)
    }

    pub fn is_saved(&self) -> bool {
        let current_id = if self.history.log_position == 0 {
            Some(0)
        } else {
            Some(self.history.log[self.history.log_position - 1].id())
        };
        current_id == self.history.last_saved_log_id
    }

    fn post_table_cells_change_hook(&mut self, table_id: u32) {
        let table = match self.spreadsheet.tables.get(table_id) {
            Some(t) => t,
            None => return,
        };
        let sheet = table.sheet_id as usize;
        let col_start = table.body_start.col;
        let col_end = table.body_end.col;
        let row_start = table.body_start.row;
        let row_end = table.body_end.row;
        let proj_id = table.projection_id;

        let proj = self.spreadsheet.projections.get_mut(proj_id).unwrap();
        let num_cols = (col_end - col_start + 1) as usize;

        for col_idx in 0..num_cols {
            let col = col_start + col_idx as u32;
            let mut old_opts = std::mem::take(&mut proj.filter_options_per_column[col_idx]);

            // reset counts, rebuild from grid
            for opt in old_opts.values_mut() {
                opt.count = 0;
            }
            for row in row_start..=row_end {
                if let Some(val) =
                    self.spreadsheet.sheets[sheet].get_value(&GridCellId { row, col })
                {
                    old_opts
                        .entry(val.clone())
                        .and_modify(|o| o.count += 1)
                        .or_insert_with(|| {
                            let id = proj.next_filter_option_id;
                            proj.next_filter_option_id += 1;
                            ProjectionFilterOption {
                                id,
                                selected: true,
                                count: 1,
                            }
                        });
                }
            }
            // remove options with count 0 (value no longer in grid)
            old_opts.retain(|_, o| o.count > 0);

            proj.filter_options_per_column[col_idx] = old_opts;
        }
    }

    fn collect_affected_cells_for_column_or_row_change(
        &mut self,
        spec: ColumnOrRowChangeSpec,
    ) -> Vec<AbsoluteCellId> {
        // references into shift range must be rewritten
        let shift_range = if spec.changing_row {
            CellRange::new(spec.sheet_id, spec.shift_start, 0, u32::MAX, u32::MAX)
        } else {
            CellRange::new(spec.sheet_id, 0, spec.shift_start, u32::MAX, u32::MAX)
        };

        // affected = dependants of shifted range + moved cells that depend on unshifted side
        let mut affected_cells = self
            .spreadsheet
            .dependency_graph
            .direct_dependants_for_range(shift_range);
        let mut seen_cells: HashSet<AbsoluteCellId> = affected_cells.iter().copied().collect();
        if spec.shift_start > 0 {
            let unshifted_range = if spec.changing_row {
                CellRange::new(spec.sheet_id, 0, 0, spec.shift_start - 1, u32::MAX)
            } else {
                CellRange::new(spec.sheet_id, 0, 0, u32::MAX, spec.shift_start - 1)
            };
            for dependant_id in self
                .spreadsheet
                .dependency_graph
                .direct_dependants_for_range(unshifted_range)
            {
                let is_moved_cell = dependant_id.sheet_id == spec.sheet_id
                    && if spec.changing_row {
                        dependant_id.row >= spec.shift_start
                    } else {
                        dependant_id.col >= spec.shift_start
                    };
                if is_moved_cell && seen_cells.insert(dependant_id) {
                    affected_cells.push(dependant_id);
                }
            }
        }

        // on remove, formulas that point directly into deleted axis must be rewritten too
        if let Some(deleted_axis) = spec.deleted_index {
            let deleted_range = if spec.changing_row {
                CellRange::new(spec.sheet_id, deleted_axis, 0, deleted_axis, u32::MAX)
            } else {
                CellRange::new(spec.sheet_id, 0, deleted_axis, u32::MAX, deleted_axis)
            };
            for dependant_id in self
                .spreadsheet
                .dependency_graph
                .direct_dependants_for_range(deleted_range)
            {
                if seen_cells.insert(dependant_id) {
                    affected_cells.push(dependant_id);
                }
            }
        }

        affected_cells
    }

    fn rewrite_formulas_for_column_or_row_change(
        &mut self,
        spec: ColumnOrRowChangeSpec,
        affected_cells: Vec<AbsoluteCellId>,
        changes: &mut Vec<CellUpdate>,
    ) {
        let mut replaced_formulas: HashMap<(FormulaId, u32), FormulaId> = HashMap::new();

        for dependant_id in affected_cells {
            let Some(dependant) = self.spreadsheet.get_cell(&dependant_id).cloned() else {
                continue;
            };
            let Some(formula_id) = dependant.defined_by_formula else {
                continue;
            };

            let source_axis = if spec.changing_row {
                dependant_id.row
            } else {
                dependant_id.col
            };
            let source_moves =
                dependant_id.sheet_id == spec.sheet_id && source_axis >= spec.shift_start;
            let cache_key = (formula_id, source_axis);
            let dependant_with_shifted_references;

            // if already replaced this formula, then just insert replacement formula's id
            if let Some(replacement_id) = replaced_formulas.get(&cache_key) {
                dependant_with_shifted_references = Cell {
                    defined_by_formula: Some(*replacement_id),
                    val: dependant.val.clone(),
                    pending_dependencies: 0,
                };
            } else {
                // otherwise, create new formula with shifted AST for structure update
                let mut formula = self
                    .spreadsheet
                    .formulas
                    .get(formula_id)
                    .expect("formula_id to exist")
                    .clone();

                for expr in formula.ast.iter_mut() {
                    let Expr::Atom(atom) = expr else {
                        continue;
                    };

                    let mut replace_with_error = false;

                    if let ExprAtom::Reference(reference) = atom {
                        let reference_range = reference.to_cell_range(&dependant_id);
                        if reference_range.sheet_id != spec.sheet_id {
                            continue;
                        }

                        if let Some(deleted_axis) = spec.deleted_index {
                            let points_to_deleted = if spec.changing_row {
                                reference_range.start_row <= deleted_axis
                                    && deleted_axis <= reference_range.end_row
                            } else {
                                reference_range.start_col <= deleted_axis
                                    && deleted_axis <= reference_range.end_col
                            };
                            if points_to_deleted {
                                replace_with_error = true;
                            }
                        }

                        if !replace_with_error {
                            // keep absolute and relative coordinates consistent after structure change
                            let source_base = if spec.changing_row {
                                dependant_id.row
                            } else {
                                dependant_id.col
                            };
                            let shifted_base = if source_moves {
                                match spec.change {
                                    ColumnOrRowChange::Insert => source_base.saturating_add(1),
                                    ColumnOrRowChange::Remove => source_base.saturating_sub(1),
                                }
                            } else {
                                source_base
                            };
                            let shift = |coord: &mut Coordinate| {
                                let source_abs = coord.to_index(source_base);
                                let shifted_abs = match spec.change {
                                    ColumnOrRowChange::Insert if source_abs >= spec.shift_start => {
                                        source_abs.saturating_add(1)
                                    }
                                    ColumnOrRowChange::Remove if source_abs >= spec.shift_start => {
                                        source_abs.saturating_sub(1)
                                    }
                                    _ => source_abs,
                                };
                                *coord = match coord {
                                    Coordinate::Absolute(_) => Coordinate::Absolute(shifted_abs),
                                    Coordinate::Relative(_) => Coordinate::Relative(
                                        (shifted_abs as i64 - shifted_base as i64)
                                            .clamp(i32::MIN as i64, i32::MAX as i64)
                                            as i32,
                                    ),
                                };
                            };

                            match reference {
                                Reference::Single { row, col, .. } => {
                                    if spec.changing_row {
                                        shift(row);
                                    } else {
                                        shift(col);
                                    }
                                }
                                Reference::Range {
                                    start_row,
                                    start_col,
                                    end_row,
                                    end_col,
                                    ..
                                } => {
                                    if spec.changing_row {
                                        shift(start_row);
                                        shift(end_row);
                                    } else {
                                        shift(start_col);
                                        shift(end_col);
                                    }
                                }
                            }
                        }
                    }

                    if replace_with_error {
                        // keep formula shape, but mark deleted reference as error atom
                        *atom = ExprAtom::InvalidReferenceError("#REF!".into());
                    }
                }

                let replacement_id = self.spreadsheet.formulas.insert(formula);
                replaced_formulas.insert(cache_key, replacement_id);
                dependant_with_shifted_references = Cell {
                    defined_by_formula: Some(replacement_id),
                    val: dependant.val.clone(),
                    pending_dependencies: 0,
                };
            }

            let old = self.set_cell(
                &dependant_id,
                Some(dependant_with_shifted_references.clone()),
            );
            changes.push(CellUpdate(
                dependant_id,
                old,
                Some(dependant_with_shifted_references),
            ));
        }
    }

    fn move_cells_for_column_or_row_change(
        &mut self,
        spec: ColumnOrRowChangeSpec,
        changes: &mut Vec<CellUpdate>,
    ) {
        let max_row = self.spreadsheet.sheets[spec.sheet_id as usize].find_biggest_row();
        let max_col = self.spreadsheet.sheets[spec.sheet_id as usize].find_biggest_column();

        match (spec.changing_row, spec.change) {
            (true, ColumnOrRowChange::Insert) => {
                // move cells from bottom to top, so each destination is free when we write into it
                if spec.shift_start <= max_row {
                    for row in (spec.shift_start..=max_row).rev() {
                        for col in 0..=max_col {
                            let source_id = AbsoluteCellId {
                                sheet_id: spec.sheet_id,
                                row,
                                col,
                            };
                            let dest_id = AbsoluteCellId {
                                sheet_id: spec.sheet_id,
                                row: row + 1,
                                col,
                            };
                            let Some(cell) = self.spreadsheet.get_cell(&source_id).cloned() else {
                                continue;
                            };

                            let old_dest = self.set_cell(&dest_id, Some(cell.clone()));
                            let old_source = self.set_cell(&source_id, None);
                            self.spreadsheet.names.move_cell_name(&source_id, &dest_id);

                            changes.push(CellUpdate(dest_id, old_dest, Some(cell)));
                            changes.push(CellUpdate(source_id, old_source, None));
                        }
                    }
                }
            }
            (false, ColumnOrRowChange::Insert) => {
                // move cells from right to left, so each destination is free when we write into it
                if spec.shift_start <= max_col {
                    for col in (spec.shift_start..=max_col).rev() {
                        for row in 0..=max_row {
                            let source_id = AbsoluteCellId {
                                sheet_id: spec.sheet_id,
                                row,
                                col,
                            };
                            let dest_id = AbsoluteCellId {
                                sheet_id: spec.sheet_id,
                                row,
                                col: col + 1,
                            };
                            let Some(cell) = self.spreadsheet.get_cell(&source_id).cloned() else {
                                continue;
                            };

                            let old_dest = self.set_cell(&dest_id, Some(cell.clone()));
                            let old_source = self.set_cell(&source_id, None);
                            self.spreadsheet.names.move_cell_name(&source_id, &dest_id);

                            changes.push(CellUpdate(dest_id, old_dest, Some(cell)));
                            changes.push(CellUpdate(source_id, old_source, None));
                        }
                    }
                }
            }
            (true, ColumnOrRowChange::Remove) => {
                // move cells from top to bottom when deleting row, so each source is read once
                if spec.shift_start > max_row {
                    // deleting the last used row: just clear deleted row
                    if spec.index <= max_row {
                        for col in 0..=max_col {
                            let deleted_id = AbsoluteCellId {
                                sheet_id: spec.sheet_id,
                                row: spec.index,
                                col,
                            };
                            let old = self.set_cell(&deleted_id, None);
                            changes.push(CellUpdate(deleted_id, old, None));
                        }
                    }
                } else {
                    for row in spec.shift_start..=max_row {
                        for col in 0..=max_col {
                            let source_id = AbsoluteCellId {
                                sheet_id: spec.sheet_id,
                                row,
                                col,
                            };
                            let dest_id = AbsoluteCellId {
                                sheet_id: spec.sheet_id,
                                row: row - 1,
                                col,
                            };
                            let Some(cell) = self.spreadsheet.get_cell(&source_id).cloned() else {
                                continue;
                            };

                            let old_dest = self.set_cell(&dest_id, Some(cell.clone()));
                            let old_source = self.set_cell(&source_id, None);
                            self.spreadsheet.names.move_cell_name(&source_id, &dest_id);

                            changes.push(CellUpdate(dest_id, old_dest, Some(cell)));
                            changes.push(CellUpdate(source_id, old_source, None));
                        }
                    }

                    for col in 0..=max_col {
                        let trailing_id = AbsoluteCellId {
                            sheet_id: spec.sheet_id,
                            row: max_row,
                            col,
                        };
                        let old = self.set_cell(&trailing_id, None);
                        changes.push(CellUpdate(trailing_id, old, None));
                    }
                }
            }
            (false, ColumnOrRowChange::Remove) => {
                // move cells from left to right when deleting column, so each source is read once
                if spec.shift_start > max_col {
                    // deleting the last used column: just clear deleted column
                    if spec.index <= max_col {
                        for row in 0..=max_row {
                            let deleted_id = AbsoluteCellId {
                                sheet_id: spec.sheet_id,
                                row,
                                col: spec.index,
                            };
                            let old = self.set_cell(&deleted_id, None);
                            changes.push(CellUpdate(deleted_id, old, None));
                        }
                    }
                } else {
                    for col in spec.shift_start..=max_col {
                        for row in 0..=max_row {
                            let source_id = AbsoluteCellId {
                                sheet_id: spec.sheet_id,
                                row,
                                col,
                            };
                            let dest_id = AbsoluteCellId {
                                sheet_id: spec.sheet_id,
                                row,
                                col: col - 1,
                            };
                            let Some(cell) = self.spreadsheet.get_cell(&source_id).cloned() else {
                                continue;
                            };

                            let old_dest = self.set_cell(&dest_id, Some(cell.clone()));
                            let old_source = self.set_cell(&source_id, None);
                            self.spreadsheet.names.move_cell_name(&source_id, &dest_id);

                            changes.push(CellUpdate(dest_id, old_dest, Some(cell)));
                            changes.push(CellUpdate(source_id, old_source, None));
                        }
                    }

                    for row in 0..=max_row {
                        let trailing_id = AbsoluteCellId {
                            sheet_id: spec.sheet_id,
                            row,
                            col: max_col,
                        };
                        let old = self.set_cell(&trailing_id, None);
                        changes.push(CellUpdate(trailing_id, old, None));
                    }
                }
            }
        }
    }

    fn apply_column_or_row_change(&mut self, spec: ColumnOrRowChangeSpec) -> Vec<CellUpdate> {
        let collect_time = Instant::now();
        let affected_cells = self.collect_affected_cells_for_column_or_row_change(spec);
        let dependants_duration = collect_time.elapsed();

        let mut changes = Vec::new();
        let formula_time = Instant::now();
        self.rewrite_formulas_for_column_or_row_change(spec, affected_cells, &mut changes);
        let formula_duration = formula_time.elapsed();

        let move_time = Instant::now();
        self.move_cells_for_column_or_row_change(spec, &mut changes);
        let move_duration = move_time.elapsed();

        debug!(
            "{} (dependencies & dependants) took: {:?}",
            spec.operation_name(),
            dependants_duration
        );
        debug!(
            "{} (rewriting formulas) took: {:?}",
            spec.operation_name(),
            formula_duration
        );
        debug!(
            "{} (moving cells) took: {:?}",
            spec.operation_name(),
            move_duration
        );

        changes
    }

    fn push_structural_history_entry(
        &mut self,
        spec: ColumnOrRowChangeSpec,
        changes: Vec<CellUpdate>,
    ) {
        let bounds = if changes.is_empty() {
            if spec.changing_row {
                ChangeBounds {
                    min_row: spec.index,
                    max_row: spec.index,
                    min_col: 0,
                    max_col: 0,
                }
            } else {
                ChangeBounds {
                    min_row: 0,
                    max_row: 0,
                    min_col: spec.index,
                    max_col: spec.index,
                }
            }
        } else {
            compute_bounds(&changes)
        };

        self.history.log.truncate(self.history.log_position);
        let id = self.history.next_log_id;
        self.history.next_log_id += 1;

        let entry = match (spec.change, spec.changing_row) {
            (ColumnOrRowChange::Insert, true) => HistoryLogEntry::InsertRow {
                id,
                changes,
                bounds,
            },
            (ColumnOrRowChange::Insert, false) => HistoryLogEntry::InsertColumn {
                id,
                changes,
                bounds,
            },
            (ColumnOrRowChange::Remove, true) => HistoryLogEntry::RemoveRow {
                id,
                changes,
                bounds,
            },
            (ColumnOrRowChange::Remove, false) => HistoryLogEntry::RemoveColumn {
                id,
                changes,
                bounds,
            },
        };
        self.history.log.push(entry);
        self.history.log_position = self.history.log.len();
    }

    pub fn insert_column_or_row(
        &mut self,
        sheet_id: u32,
        inserting_row: bool,
        index: u32,
        include_index: bool,
    ) {
        // one path for insert, split by row/column and include_index inside spec
        let spec = ColumnOrRowChangeSpec::for_insert(sheet_id, inserting_row, index, include_index);
        let changes = self.apply_column_or_row_change(spec);
        self.push_structural_history_entry(spec, changes);
        self.history.last_saved_log_id = None;
    }

    pub fn remove_column_or_row(&mut self, sheet_id: u32, removing_row: bool, index: u32) {
        // one path for remove, split by row/column inside spec
        let spec = ColumnOrRowChangeSpec::for_remove(sheet_id, removing_row, index);
        let changes = self.apply_column_or_row_change(spec);
        self.push_structural_history_entry(spec, changes.clone());
        if !changes.is_empty() {
            self.post_cell_changes_hook(changes);
        }
        self.history.last_saved_log_id = None;
    }

    /// Sort table by column by reordering the current projected rows.
    pub fn sort_table_column(
        &mut self,
        table_id: u32,
        sort_col: u32,
        desc: bool,
    ) -> Result<(), String> {
        let table = self
            .spreadsheet
            .tables
            .get(table_id)
            .ok_or("Table not found")?;
        let sheet = table.sheet_id as usize;
        let proj_id = table.projection_id;

        let sheets = &self.spreadsheet.sheets;
        let proj = self.spreadsheet.projections.get_mut(proj_id).unwrap();
        proj.projected_rows.sort_by(|&a, &b| {
            let va = sheets[sheet].get_value(&GridCellId {
                row: a,
                col: sort_col,
            });
            let vb = sheets[sheet].get_value(&GridCellId {
                row: b,
                col: sort_col,
            });
            // empty cells always sink to the bottom, regardless of sort direction
            match (va, vb) {
                (None, None) => std::cmp::Ordering::Equal,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (Some(_), None) => std::cmp::Ordering::Less,
                (Some(va), Some(vb)) => {
                    let ord = match (va, vb) {
                        (CellValue::Number(n1), CellValue::Number(n2)) => n1.cmp(n2),
                        (CellValue::Text(t1), CellValue::Text(t2)) => t1.cmp(t2),
                        (CellValue::Number(_), _) => std::cmp::Ordering::Less,
                        (_, CellValue::Number(_)) => std::cmp::Ordering::Greater,
                        _ => std::cmp::Ordering::Equal,
                    };
                    if desc {
                        ord.reverse()
                    } else {
                        ord
                    }
                }
            }
        });
        proj.active = true;
        Ok(())
    }

    /// Toggle a filter value for a table column. Returns hidden_rows_count.
    pub fn filter_table_column(
        &mut self,
        table_id: u32,
        filter_col: u32,
        filter_option_id: u32,
    ) -> Result<u32, String> {
        let table = self
            .spreadsheet
            .tables
            .get(table_id)
            .ok_or("Table not found")?;
        let col_start = table.body_start.col;
        let proj_id = table.projection_id;
        let proj = self.spreadsheet.projections.get_mut(proj_id).unwrap();
        let col_idx = (filter_col - col_start) as usize;

        if filter_option_id == 0 {
            proj.filter_show_blanks[col_idx] = !proj.filter_show_blanks[col_idx];
        } else if let Some(opt) = proj.filter_options_per_column[col_idx]
            .values_mut()
            .find(|o| o.id == filter_option_id)
        {
            opt.selected = !opt.selected;
        }

        self.rebuild_table_projection(table_id)
    }

    /// Select all filter options for a table column. Returns hidden_rows_count.
    pub fn select_all_column_filters(
        &mut self,
        table_id: u32,
        filter_col: u32,
    ) -> Result<u32, String> {
        let table = self
            .spreadsheet
            .tables
            .get(table_id)
            .ok_or("Table not found")?;
        let col_start = table.body_start.col;
        let proj_id = table.projection_id;
        let proj = self.spreadsheet.projections.get_mut(proj_id).unwrap();
        let col_idx = (filter_col - col_start) as usize;

        proj.filter_show_blanks[col_idx] = true;
        for opt in proj.filter_options_per_column[col_idx].values_mut() {
            opt.selected = true;
        }

        self.rebuild_table_projection(table_id)
    }

    /// Clear all filter options for a table column. Returns hidden_rows_count.
    pub fn clear_all_column_filters(
        &mut self,
        table_id: u32,
        filter_col: u32,
    ) -> Result<u32, String> {
        let table = self
            .spreadsheet
            .tables
            .get(table_id)
            .ok_or("Table not found")?;
        let col_start = table.body_start.col;
        let proj_id = table.projection_id;
        let proj = self.spreadsheet.projections.get_mut(proj_id).unwrap();
        let col_idx = (filter_col - col_start) as usize;

        proj.filter_show_blanks[col_idx] = false;
        for opt in proj.filter_options_per_column[col_idx].values_mut() {
            opt.selected = false;
        }

        self.rebuild_table_projection(table_id)
    }

    /// Rebuild projected_rows for a table from current filter state. Returns hidden_rows_count.
    fn rebuild_table_projection(&mut self, table_id: u32) -> Result<u32, String> {
        let table = self
            .spreadsheet
            .tables
            .get(table_id)
            .ok_or("Table not found")?;
        let sheet = table.sheet_id as usize;
        let col_start = table.body_start.col;
        let body_start_row = table.body_start.row;
        let body_end_row = table.body_end.row;
        let proj_id = table.projection_id;

        let sheets = &self.spreadsheet.sheets;
        let proj = self.spreadsheet.projections.get_mut(proj_id).unwrap();

        // rebuild projected_rows from all body rows with current filters
        let mut rows: Vec<u32> = (body_start_row..=body_end_row).collect();
        for (i, filter_opts) in proj.filter_options_per_column.iter().enumerate() {
            let col_blanks = proj.filter_show_blanks[i];
            if col_blanks && filter_opts.values().all(|o| o.selected) {
                continue;
            }
            let col = col_start + i as u32;
            rows.retain(
                |&row| match sheets[sheet].get_value(&GridCellId { row, col }) {
                    Some(v) => filter_opts.get(v).map_or(false, |o| o.selected),
                    None => col_blanks,
                },
            );
        }

        let total = (body_end_row - body_start_row + 1) as u32;
        proj.hidden_rows_count = total - rows.len() as u32;
        proj.projected_rows = rows;
        proj.active = proj.filter_show_blanks.iter().any(|&b| !b)
            || proj
                .filter_options_per_column
                .iter()
                .any(|col| col.values().any(|o| !o.selected));
        Ok(proj.hidden_rows_count)
    }

    /// Reset engine with an empty spreadsheet.
    pub fn create_empty_spreadsheet(&mut self) {
        self.spreadsheet = Spreadsheet::new();
        self.history = History::new();
        self.batch.clear();
    }

    /// Load a spreadsheet from disk and reset engine state.
    /// Returns the UI decorations JSON string (empty if old format).
    pub fn open_spreadsheet(&mut self, path: &str) -> io::Result<String> {
        let (spreadsheet, decorations) = file_api::load(path)?;
        self.spreadsheet = spreadsheet;

        let dependency_graph_time = Instant::now();
        self.spreadsheet.rebuild_dependency_graph();
        info!(
            "open_spreadsheet: dependency graph built in {:?}",
            dependency_graph_time.elapsed()
        );

        self.history = History::new();
        self.batch.clear();
        self.mark_saved();
        Ok(decorations)
    }

    /// Save the current spreadsheet and UI decorations to disk.
    pub fn save_spreadsheet(&mut self, path: &str, decorations_json: &str) -> io::Result<()> {
        file_api::save(&self.spreadsheet, decorations_json, path)?;
        self.mark_saved();
        Ok(())
    }

    pub fn mark_saved(&mut self) {
        self.history.last_saved_log_id = if self.history.log_position == 0 {
            Some(0)
        } else {
            Some(self.history.log[self.history.log_position - 1].id())
        };
    }
}

/// Resolve an ExprAtom to a Decimal number. If it's a single cell reference, look up the cell value.
fn resolve_number(
    source_cell: &AbsoluteCellId,
    expr: &ExprAtom,
    sheets: &Sheets,
) -> Result<Decimal, EvalError> {
    match expr {
        ExprAtom::Number(n) => Ok(*n),
        ExprAtom::InvalidReferenceError(msg) => Err(EvalError::Error(msg.clone())),
        ExprAtom::Reference(r) => match r {
            Reference::Single { sheet_id, row, col } => {
                let row = row.to_index(source_cell.row);
                let col = col.to_index(source_cell.col);
                let gid = GridCellId { row, col };
                match sheets[*sheet_id as usize].get_value(&gid) {
                    Some(CellValue::Number(n)) => Ok(*n),
                    Some(CellValue::Text(_)) => Err(EvalError::TypeError {
                        expected: AtomType::Number,
                        got: AtomType::Text,
                    }),
                    Some(CellValue::Error(msg)) => Err(EvalError::Error(msg.clone())),
                    None => Ok(Decimal::ZERO),
                }
            }
            Reference::Range { .. } => Err(EvalError::TypeError {
                expected: AtomType::Number,
                got: AtomType::Reference,
            }),
        },
        _ => Err(EvalError::TypeError {
            expected: AtomType::Number,
            got: match expr {
                ExprAtom::Boolean(_) => AtomType::Boolean,
                ExprAtom::Text(_) => AtomType::Text,
                ExprAtom::InvalidReferenceError(_) => AtomType::InvalidReferenceError,
                ExprAtom::Function(_) => AtomType::Function,
                _ => unreachable!(),
            },
        }),
    }
}

/// Resolve an ExprAtom to a range iterator. Returns (sheet_id, start_row, start_col, end_row, end_col).
fn resolve_range(
    source_cell: &AbsoluteCellId,
    expr: &ExprAtom,
    _sheets: &Sheets,
) -> Result<(SheetId, u32, u32, u32, u32), EvalError> {
    match expr {
        ExprAtom::InvalidReferenceError(msg) => Err(EvalError::Error(msg.clone())),
        ExprAtom::Reference(Reference::Range {
            sheet_id,
            start_row,
            start_col,
            end_row,
            end_col,
        }) => {
            let sr = start_row.to_index(source_cell.row);
            let sc = start_col.to_index(source_cell.col);
            let er = end_row.to_index(source_cell.row);
            let ec = end_col.to_index(source_cell.col);
            Ok((*sheet_id, sr.min(er), sc.min(ec), sr.max(er), sc.max(ec)))
        }
        _ => Err(EvalError::TypeError {
            expected: AtomType::Reference,
            got: match expr {
                ExprAtom::Boolean(_) => AtomType::Boolean,
                ExprAtom::Number(_) => AtomType::Number,
                ExprAtom::Text(_) => AtomType::Text,
                ExprAtom::InvalidReferenceError(_) => AtomType::InvalidReferenceError,
                ExprAtom::Function(_) => AtomType::Function,
                ExprAtom::Reference(Reference::Single { .. }) => AtomType::Reference,
                _ => unreachable!(),
            },
        }),
    }
}

/// Evaluate a single formula's AST, returning the computed CellValue.
fn eval_formula(
    ast: &[Expr],
    source_cell: &AbsoluteCellId,
    sheets: &Sheets,
    eval_store: &mut Vec<ExprAtom>,
) -> Result<CellValue, EvalError> {
    eval_store.clear();

    for expr in ast {
        let res = match expr {
            Expr::Atom(atom) => atom.clone(),
            Expr::Negate(id) => {
                let n = resolve_number(source_cell, &eval_store[*id as usize], sheets)?;
                ExprAtom::Number(-n)
            }
            Expr::Add(a_id, b_id) => {
                let a = resolve_number(source_cell, &eval_store[*a_id as usize], sheets)?;
                let b = resolve_number(source_cell, &eval_store[*b_id as usize], sheets)?;
                ExprAtom::Number(a + b)
            }
            Expr::Subtract(a_id, b_id) => {
                let a = resolve_number(source_cell, &eval_store[*a_id as usize], sheets)?;
                let b = resolve_number(source_cell, &eval_store[*b_id as usize], sheets)?;
                ExprAtom::Number(a - b)
            }
            Expr::Multiply(a_id, b_id) => {
                let a = resolve_number(source_cell, &eval_store[*a_id as usize], sheets)?;
                let b = resolve_number(source_cell, &eval_store[*b_id as usize], sheets)?;
                ExprAtom::Number(a * b)
            }
            Expr::Divide(a_id, b_id) => {
                let a = resolve_number(source_cell, &eval_store[*a_id as usize], sheets)?;
                let b = resolve_number(source_cell, &eval_store[*b_id as usize], sheets)?;
                ExprAtom::Number(a / b)
            }
            Expr::Sum => {
                let range_arg_idx = eval_store.len() - 1;
                let (sheet_id, sr, sc, er, ec) =
                    resolve_range(source_cell, &eval_store[range_arg_idx], sheets)?;
                let sheet = &sheets[sheet_id as usize];
                let mut sum = Decimal::ZERO;
                for row in sr..=er {
                    for col in sc..=ec {
                        let gid = GridCellId { row, col };
                        if let Some(CellValue::Number(n)) = sheet.get_value(&gid) {
                            sum += *n;
                        }
                    }
                }
                ExprAtom::Number(sum)
            }
            Expr::Avg => {
                let range_arg_idx = eval_store.len() - 1;
                let (sheet_id, sr, sc, er, ec) =
                    resolve_range(source_cell, &eval_store[range_arg_idx], sheets)?;
                let sheet = &sheets[sheet_id as usize];
                let mut sum = Decimal::ZERO;
                let mut count: u64 = 0;
                for row in sr..=er {
                    for col in sc..=ec {
                        let gid = GridCellId { row, col };
                        if let Some(CellValue::Number(n)) = sheet.get_value(&gid) {
                            sum += *n;
                            count += 1;
                        }
                    }
                }
                if count == 0 {
                    ExprAtom::Number(Decimal::ZERO)
                } else {
                    ExprAtom::Number(sum / Decimal::from(count))
                }
            }
            Expr::ExtrnalFunctionCall { .. } => todo!(),
        };
        eval_store.push(res);
    }

    let value = match eval_store.pop() {
        Some(ExprAtom::Number(n)) => CellValue::Number(n),
        Some(ExprAtom::Text(s)) => CellValue::Text(s),
        Some(ExprAtom::InvalidReferenceError(s)) => CellValue::Error(s),
        Some(ExprAtom::Boolean(b)) => CellValue::Text(b.to_string()),
        Some(other) => CellValue::Text(format!("{:?}", other)),
        None => unreachable!(),
    };

    Ok(value)
}

fn compute_bounds(changes: &[CellUpdate]) -> ChangeBounds {
    let mut min_row = u32::MAX;
    let mut max_row = 0u32;
    let mut min_col = u32::MAX;
    let mut max_col = 0u32;
    for CellUpdate(id, _, _) in changes {
        min_row = min_row.min(id.row);
        max_row = max_row.max(id.row);
        min_col = min_col.min(id.col);
        max_col = max_col.max(id.col);
    }
    ChangeBounds {
        min_row,
        max_row,
        min_col,
        max_col,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    fn cell(row: u32, col: u32) -> AbsoluteCellId {
        AbsoluteCellId {
            sheet_id: 0,
            row,
            col,
        }
    }

    fn assert_cycle_error(engine: &Engine, id: AbsoluteCellId) {
        let value = engine
            .spreadsheet
            .get_cell(&id)
            .map(|cell| cell.val.clone());
        assert_eq!(value, Some(CellValue::Error("Cycle".into())));
    }

    #[test]
    fn direct_cycle_sets_cycle_error() {
        let mut engine = Engine::new();
        let guard = engine.start_batch();

        engine.parse_and_insert_string(&guard, cell(0, 0), "=B1");
        engine.parse_and_insert_string(&guard, cell(0, 1), "=A1");
        engine.end_batch(guard);

        assert_cycle_error(&engine, cell(0, 0));
        assert_cycle_error(&engine, cell(0, 1));
    }

    #[test]
    fn formulas_blocked_by_cycle_also_get_cycle_error() {
        let mut engine = Engine::new();
        let guard = engine.start_batch();

        engine.parse_and_insert_string(&guard, cell(0, 0), "=B1");
        engine.parse_and_insert_string(&guard, cell(0, 1), "=A1");
        engine.parse_and_insert_string(&guard, cell(1, 0), "=A1");
        engine.end_batch(guard);

        assert_cycle_error(&engine, cell(0, 0));
        assert_cycle_error(&engine, cell(0, 1));
        assert_cycle_error(&engine, cell(1, 0));
    }

    #[test]
    fn remove_column_removes_selected_column() {
        let mut engine = Engine::new();
        let guard = engine.start_batch();
        engine.insert_value(&guard, cell(0, 0), CellValue::Text("A".into()));
        engine.insert_value(&guard, cell(0, 1), CellValue::Text("B".into()));
        engine.insert_value(&guard, cell(0, 2), CellValue::Text("C".into()));
        engine.end_batch(guard);

        engine.remove_column_or_row(0, false, 1);

        assert!(matches!(
            engine.spreadsheet.get_cell(&cell(0, 0)).map(|c| &c.val),
            Some(CellValue::Text(v)) if v == "A"
        ));
        assert!(matches!(
            engine.spreadsheet.get_cell(&cell(0, 1)).map(|c| &c.val),
            Some(CellValue::Text(v)) if v == "C"
        ));
        assert!(engine.spreadsheet.get_cell(&cell(0, 2)).is_none());
    }

    #[test]
    fn remove_row_removes_selected_row() {
        let mut engine = Engine::new();
        let guard = engine.start_batch();
        engine.insert_value(&guard, cell(0, 0), CellValue::Text("R0".into()));
        engine.insert_value(&guard, cell(1, 0), CellValue::Text("R1".into()));
        engine.insert_value(&guard, cell(2, 0), CellValue::Text("R2".into()));
        engine.end_batch(guard);

        engine.remove_column_or_row(0, true, 1);

        assert!(matches!(
            engine.spreadsheet.get_cell(&cell(0, 0)).map(|c| &c.val),
            Some(CellValue::Text(v)) if v == "R0"
        ));
        assert!(matches!(
            engine.spreadsheet.get_cell(&cell(1, 0)).map(|c| &c.val),
            Some(CellValue::Text(v)) if v == "R2"
        ));
        assert!(engine.spreadsheet.get_cell(&cell(2, 0)).is_none());
    }

    #[test]
    fn undo_redo_insert_row_reverts_and_reapplies_structure() {
        let mut engine = Engine::new();
        let guard = engine.start_batch();
        engine.insert_value(&guard, cell(0, 0), CellValue::Text("R0".into()));
        engine.insert_value(&guard, cell(1, 0), CellValue::Text("R1".into()));
        engine.end_batch(guard);

        engine.insert_column_or_row(0, true, 1, true);
        assert!(engine.spreadsheet.get_cell(&cell(1, 0)).is_none());
        assert!(matches!(
            engine.spreadsheet.get_cell(&cell(2, 0)).map(|c| &c.val),
            Some(CellValue::Text(v)) if v == "R1"
        ));

        engine.undo();
        assert!(matches!(
            engine.spreadsheet.get_cell(&cell(1, 0)).map(|c| &c.val),
            Some(CellValue::Text(v)) if v == "R1"
        ));
        assert!(engine.spreadsheet.get_cell(&cell(2, 0)).is_none());

        engine.redo();
        assert!(engine.spreadsheet.get_cell(&cell(1, 0)).is_none());
        assert!(matches!(
            engine.spreadsheet.get_cell(&cell(2, 0)).map(|c| &c.val),
            Some(CellValue::Text(v)) if v == "R1"
        ));
    }

    #[test]
    fn remove_last_column_rewrites_deleted_reference_to_error_atom() {
        let mut engine = Engine::new();
        let guard = engine.start_batch();
        engine.parse_and_insert_string(&guard, cell(5, 6), "5");
        engine.parse_and_insert_string(&guard, cell(5, 5), "=G6+5");
        engine.end_batch(guard);

        let old_formula_id = engine
            .spreadsheet
            .get_cell(&cell(5, 5))
            .and_then(|c| c.defined_by_formula)
            .expect("formula in F6");
        let old_len = engine
            .spreadsheet
            .formulas
            .get(old_formula_id)
            .expect("formula to exist")
            .ast
            .len();

        engine.remove_column_or_row(0, false, 6);

        let formula_cell = engine
            .spreadsheet
            .get_cell(&cell(5, 5))
            .expect("F6 cell to exist");
        assert_eq!(formula_cell.val, CellValue::Error("#REF!".into()));

        let new_formula_id = formula_cell.defined_by_formula.expect("formula id");
        let new_formula = engine
            .spreadsheet
            .formulas
            .get(new_formula_id)
            .expect("formula to exist");
        assert_eq!(new_formula.ast.len(), old_len);
        assert!(new_formula
            .ast
            .iter()
            .any(|expr| matches!(expr, Expr::Atom(ExprAtom::InvalidReferenceError(msg)) if msg == "#REF!")));

        assert!(engine.spreadsheet.get_cell(&cell(5, 6)).is_none());
    }

    #[test]
    #[ignore]
    fn bench_eval_insert_and_recalc() {
        const ROWS: u32 = 400;
        const DATA_COLS: u32 = 12; // A..L
        const EDITS: u32 = 500;

        let mut engine = Engine::new();

        // create dense numeric block + one row sum per row (M) + one global sum (N)
        let guard = engine.start_batch();
        for row in 0..ROWS {
            for col in 0..DATA_COLS {
                engine.insert_number(&guard, cell(row, col), Decimal::from((row + col) as i64));
            }
            engine.parse_and_insert_string(
                &guard,
                cell(row, DATA_COLS),
                &format!("=SUM(A{}:L{})", row + 1, row + 1),
            );
        }
        engine.parse_and_insert_string(
            &guard,
            cell(ROWS, DATA_COLS + 1),
            &format!("=SUM(M1:M{})", ROWS),
        );
        engine.end_batch(guard);

        // warm up to stabilize caches / branch predictors
        for i in 0..50 {
            let row = i % ROWS;
            let g = engine.start_batch();
            engine.insert_number(&g, cell(row, 0), Decimal::from((i * 3) as i64));
            engine.end_batch(g);
        }

        // benchmark steady-state edit + recalculation path
        let timer = Instant::now();
        for i in 0..EDITS {
            let row = i % ROWS;
            let g = engine.start_batch();
            engine.insert_number(&g, cell(row, 0), Decimal::from((i * 7) as i64));
            engine.end_batch(g);
        }
        let elapsed = timer.elapsed();

        eprintln!(
            "bench_eval_insert_and_recalc: edits={}, total={:?}, per_edit={:?}",
            EDITS,
            elapsed,
            elapsed / EDITS
        );
    }
}
