use std::marker::PhantomData;

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use tauri_plugin_log::log::debug;

use std::io;

use crate::file_api;
use crate::parser::{lex_formula, parse_formula, FormulaState};
use crate::storage::grid::{Cell, CellValue, GridCellId};
use crate::storage::types::{
    AbsoluteCellId, AtomType, Expr, ExprAtom, Formula, FormulaId, ProjectionFilterOption,
    Reference, SheetId, Sheets, Spreadsheet,
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

struct HistoryLogEntry {
    id: u64,
    changes: Vec<CellUpdate>,
    bounds: ChangeBounds,
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
        debug!("Eval #{}", self.debug.eval_number);
        let dep_time = std::time::Instant::now();

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
                    let t = std::time::Instant::now();
                    let new_value = match eval_formula(
                        ast,
                        &cell_id,
                        &self.spreadsheet.sheets,
                        &mut eval_store,
                    ) {
                        Ok(v) => v,
                        Err(e) => CellValue::Error(format!("{:?}", e)),
                    };
                    eval_duration += t.elapsed();

                    self.spreadsheet.set_value(&cell_id, new_value);
                }
            }

            // Decrement pending_dependencies_count of dependents, add to wave if ready
            let t = std::time::Instant::now();
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
        let t = std::time::Instant::now();
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

        let table_time = std::time::Instant::now();
        for table_id in affected_tables {
            self.post_table_cells_change_hook(table_id);
        }

        debug!("Eval (dependencies & dependants) took: {:?}", dep_duration);
        debug!("Eval (running expressions) took: {:?}", eval_duration);
        debug!("Eval (updating tables) took: {:?}", table_time.elapsed());
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

        self.history.log.push(HistoryLogEntry {
            id,
            changes,
            bounds,
        });
        self.history.log_position = self.history.log.len();

        self.post_cell_changes_hook(eval_changes);
    }

    pub fn undo(&mut self) -> Option<ChangeBounds> {
        if self.history.log_position == 0 {
            return None;
        }
        self.history.log_position -= 1;
        let entry = &self.history.log[self.history.log_position];
        let bounds = entry.bounds.clone();
        let changes = entry.changes.clone();
        for CellUpdate(id, old, _) in &changes {
            self.set_cell(id, old.clone());
        }
        self.post_cell_changes_hook(changes);
        Some(bounds)
    }

    pub fn redo(&mut self) -> Option<ChangeBounds> {
        if self.history.log_position >= self.history.log.len() {
            return None;
        }
        let entry = &self.history.log[self.history.log_position];
        let bounds = entry.bounds.clone();
        let changes = entry.changes.clone();
        self.history.log_position += 1;
        for CellUpdate(id, _, new) in &changes {
            self.set_cell(id, new.clone());
        }
        self.post_cell_changes_hook(changes);
        Some(bounds)
    }

    pub fn is_saved(&self) -> bool {
        let current_id = if self.history.log_position == 0 {
            Some(0)
        } else {
            Some(self.history.log[self.history.log_position - 1].id)
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
        self.spreadsheet.rebuild_dependency_graph();
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
            Some(self.history.log[self.history.log_position - 1].id)
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
        ExprAtom::Reference(r) => match r {
            Reference::Single { sheet_id, row, col } => {
                let row = row.to_index(source_cell.row);
                let col = col.to_index(source_cell.col);
                let gid = GridCellId { row, col };
                match sheets[*sheet_id as usize].get_value(&gid) {
                    Some(CellValue::Number(n)) => Ok(*n),
                    Some(_) => Err(EvalError::TypeError {
                        expected: AtomType::Number,
                        got: AtomType::Text,
                    }),
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
}
