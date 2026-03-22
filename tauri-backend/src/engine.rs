use std::marker::PhantomData;

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use tauri_plugin_log::log::debug;

use std::io;

use crate::file_api;
use crate::parser::{lex_formula, parse_formula, FormulaState};
use crate::storage::grid::{CellContent, CellValue, GridCellId};
use crate::storage::types::{
    AbsoluteCellId, AtomType, Expr, ExprAtom, Formula, FormulaId, ProjectionFilterOption,
    Reference, SheetId, Sheets, Spreadsheet,
};

/// A single cell mutation: (cell_id, old_value, new_value).
#[derive(Clone)]
pub struct CellUpdate(
    pub AbsoluteCellId,
    pub Option<CellContent>,
    pub Option<CellContent>,
);

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

    /// If the cell is defined by a formula, remove this cell from the dependants of its dependencies
    fn remove_cell_from_dependents(&mut self, id: &AbsoluteCellId) {
        let Some(content) = self.spreadsheet.get_content(id) else {
            return;
        };
        let Some(dependencies) = content.dependencies.clone() else {
            return;
        };
        for dep in &dependencies {
            self.spreadsheet.remove_dependant(dep, id);
        }
    }

    /// If the cell is defined by a formula, add this cell to the dependants of its dependencies
    fn add_cell_to_dependents(&mut self, id: &AbsoluteCellId) {
        let Some(content) = self.spreadsheet.get_content(id) else {
            return;
        };
        let Some(dependencies) = content.dependencies.clone() else {
            return;
        };
        for dep in &dependencies {
            self.spreadsheet.add_dependant(dep, id);
        }
    }

    /// Parse string and insert appropriate cell.
    pub fn parse_and_insert_string(&mut self, _: &EngineGuard, id: AbsoluteCellId, input: &str) {
        let old = self.spreadsheet.get_content(&id).cloned();

        // Remove this cell from dependants of its old dependencies
        self.remove_cell_from_dependents(&id);

        // if not entering formula, just update value
        if !input.starts_with('=') {
            let new = if let Ok(n) = input.parse::<Decimal>() {
                CellContent::number(n)
            } else {
                CellContent::text(input.to_string())
            };
            self.spreadsheet.insert_content(&id, new.clone());
            self.batch.push(CellUpdate(id, old, Some(new)));
            return;
        }

        // start parsing formula
        let formula_text = &input[1..];
        let lex_result = lex_formula(formula_text);
        // report any errors during lexing
        if lex_result.has_errors() {
            let new = CellContent::error("Lex error".into());
            self.spreadsheet.insert_content(&id, new.clone());
            self.batch.push(CellUpdate(id, old, Some(new)));
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
            dependencies: Vec::new(),
        };
        let (parsed, parse_errs) = parse_formula(&tokens, formula_text.len(), &mut state);
        // report any errors during parsing (syntax, name not found)
        if !parse_errs.is_empty() {
            let new = CellContent::error("Parse error".into());
            self.spreadsheet.insert_content(&id, new.clone());
            self.batch.push(CellUpdate(id, old, Some(new)));
            return;
        }
        let Some((ast, _root_id)) = parsed else {
            return;
        };

        let dependencies = state.dependencies;

        // create and store formula
        let formula = Formula {
            ast,
            formula_string: input.to_string(),
        };
        let formula_id = self.spreadsheet.formulas.insert(formula);

        // insert cell with formula reference and dependencies
        let new = CellContent {
            defined_by_formula: Some(formula_id),
            dependencies: if dependencies.is_empty() {
                None
            } else {
                Some(dependencies)
            },
            val: CellValue::Error(String::new()),
            pending_dependencies: 0,
        };
        self.spreadsheet.insert_content(&id, new.clone());

        // add this cell as dependant to all its dependencies
        self.add_cell_to_dependents(&id);

        self.batch.push(CellUpdate(id, old, Some(new)));
    }

    pub fn insert_number(&mut self, _: &EngineGuard, id: AbsoluteCellId, n: Decimal) {
        let old = self.spreadsheet.get_content(&id).cloned();
        self.remove_cell_from_dependents(&id);
        let new = CellContent::number(n);
        self.spreadsheet.insert_content(&id, new.clone());
        self.batch.push(CellUpdate(id, old, Some(new)));
    }

    /// Insert a cell that shares an existing formula. Resolves dependencies from the
    /// formula's AST relative to the new cell position.
    pub fn insert_shared_formula(
        &mut self,
        _: &EngineGuard,
        id: AbsoluteCellId,
        formula_id: FormulaId,
    ) {
        let old = self.spreadsheet.get_content(&id).cloned();
        self.remove_cell_from_dependents(&id);

        let dependencies = self
            .spreadsheet
            .formulas
            .get(formula_id)
            .map(|f| find_dependencies_from_relative_references(&f.ast, &id))
            .unwrap_or_default();

        let new = CellContent {
            defined_by_formula: Some(formula_id),
            dependencies: if dependencies.is_empty() {
                None
            } else {
                Some(dependencies)
            },
            val: CellValue::Error(String::new()),
            pending_dependencies: 0,
        };
        self.spreadsheet.insert_content(&id, new.clone());
        self.add_cell_to_dependents(&id);
        self.batch.push(CellUpdate(id, old, Some(new)));
    }

    pub fn delete(&mut self, _: &EngineGuard, id: AbsoluteCellId) {
        let old = self.spreadsheet.get_content(&id).cloned();
        self.remove_cell_from_dependents(&id);
        self.spreadsheet.remove_content(&id);
        self.batch.push(CellUpdate(id, old, None));
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

        let change_set: std::collections::HashSet<AbsoluteCellId> =
            changes.iter().map(|CellUpdate(id, _, _)| *id).collect();
        let mut current = changes.clone();
        let mut next: Vec<CellUpdate> = Vec::new();
        while !current.is_empty() {
            for CellUpdate(cell_id, _, _) in &current {
                let dependents = self.spreadsheet.get_dependents(cell_id).cloned();
                if let Some(dependents) = dependents {
                    for d in dependents {
                        let count = self.spreadsheet.get_pending_dependencies(&d);
                        if count == 0 && !change_set.contains(&d) {
                            next.push(CellUpdate(d, None, None));
                        }
                        self.spreadsheet.increase_pending_dependencies(&d);
                    }
                }
            }

            current.clear();
            std::mem::swap(&mut current, &mut next);
        }

        // add cells, that have their dependencies already calculated into "wave"
        let mut wave: Vec<CellUpdate> = Vec::new();
        for CellUpdate(cell_id, _, _) in &changes {
            if self.spreadsheet.get_pending_dependencies(cell_id) == 0 {
                wave.push(CellUpdate(*cell_id, None, None));
            }
        }

        let mut dep_duration = dep_time.elapsed();
        let mut eval_duration = std::time::Duration::ZERO;
        let mut eval_store: Vec<ExprAtom> = Vec::new();

        // step 2.
        //
        // todo

        while let Some(CellUpdate(cell_id, _, _)) = wave.pop() {
            let formula_id = self
                .spreadsheet
                .get_content(&cell_id)
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
            let deps = self.spreadsheet.get_dependents(&cell_id).cloned();
            if let Some(deps) = deps {
                for dep in deps {
                    self.spreadsheet.decrease_pending_dependencies(&dep);
                    let count = self.spreadsheet.get_pending_dependencies(&dep);
                    if count == 0 {
                        wave.push(CellUpdate(dep, None, None));
                    }
                }
            }

            self.update_table_on_cell_change(&cell_id);

            dep_duration += t.elapsed();
        }

        debug!("Eval (dependencies & dependants) took: {:?}", dep_duration);
        debug!("Eval (running expressions) took: {:?}", eval_duration);
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
            self.remove_cell_from_dependents(id);
            match old {
                Some(content) => {
                    self.spreadsheet.insert_content(id, content.clone());
                    self.add_cell_to_dependents(id);
                }
                None => self.spreadsheet.remove_content(id),
            }
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
            self.remove_cell_from_dependents(id);
            match new {
                Some(content) => {
                    self.spreadsheet.insert_content(id, content.clone());
                    self.add_cell_to_dependents(id);
                }
                None => self.spreadsheet.remove_content(id),
            }
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

    /// Add new cell value to filter options if not already present.
    fn update_table_on_cell_change(&mut self, cell_id: &AbsoluteCellId) {
        if let Some((_, table)) = self.spreadsheet.find_table_containing_cell(cell_id) {
            let proj_id = table.projection_id;
            let col_idx = (cell_id.col - table.body_start.col) as usize;
            if let Some(new_val) = self.spreadsheet.get_value(cell_id).cloned() {
                let proj = self.spreadsheet.projections.get_mut(proj_id).unwrap();
                if col_idx < proj.filter_options_per_column.len() {
                    let opts = &mut proj.filter_options_per_column[col_idx];
                    if !opts.iter().any(|o| o.val == new_val) {
                        opts.push(ProjectionFilterOption {
                            val: new_val,
                            selected: true,
                        });
                    }
                }
            }
        }
    }

    /// Sort table by column. Only one sort can be active at a time.
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
        let col_idx = (sort_col - table.body_start.col) as usize;

        let sheets = &self.spreadsheet.sheets;
        let proj = self.spreadsheet.projections.get_mut(proj_id).unwrap();
        for (i, opt) in proj.sorting_options_per_column.iter_mut().enumerate() {
            opt.selected = i == col_idx;
            opt.desc = if i == col_idx { desc } else { false };
        }
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
        filter_index: usize,
    ) -> Result<u32, String> {
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
        let col_idx = (filter_col - col_start) as usize;

        if let Some(opt) = proj
            .filter_options_per_column
            .get_mut(col_idx)
            .and_then(|opts| opts.get_mut(filter_index))
        {
            opt.selected = !opt.selected;
        }

        // Rebuild projected_rows from all body rows with current filters
        let mut rows: Vec<u32> = (body_start_row..=body_end_row).collect();
        for (i, filter_opts) in proj.filter_options_per_column.iter().enumerate() {
            if filter_opts.iter().all(|o| o.selected) {
                continue;
            }
            let col = col_start + i as u32;
            let selected: Vec<&CellValue> = filter_opts
                .iter()
                .filter(|o| o.selected)
                .map(|o| &o.val)
                .collect();
            rows.retain(|&row| {
                sheets[sheet]
                    .get_value(&GridCellId { row, col })
                    .map_or(true, |v| selected.contains(&v))
            });
        }

        // Re-apply sort if active
        if let Some((i, sort_opt)) = proj
            .sorting_options_per_column
            .iter()
            .enumerate()
            .find(|(_, o)| o.selected)
        {
            let sort_col = col_start + i as u32;
            let desc = sort_opt.desc;
            rows.sort_by(|&a, &b| {
                let va = sheets[sheet].get_value(&GridCellId {
                    row: a,
                    col: sort_col,
                });
                let vb = sheets[sheet].get_value(&GridCellId {
                    row: b,
                    col: sort_col,
                });
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
        }

        let total = (body_end_row - body_start_row + 1) as u32;
        proj.hidden_rows_count = total - rows.len() as u32;
        proj.projected_rows = rows;
        proj.active = proj.sorting_options_per_column.iter().any(|o| o.selected)
            || proj
                .filter_options_per_column
                .iter()
                .any(|col| col.iter().any(|o| !o.selected));
        Ok(proj.hidden_rows_count)
    }

    /// Reset engine with an empty spreadsheet.
    pub fn create_empty_spreadsheet(&mut self) {
        self.spreadsheet = Spreadsheet::new();
        self.history = History::new();
        self.batch.clear();
    }

    /// Load a spreadsheet from disk and reset engine state.
    pub fn open_spreadsheet(&mut self, path: &str) -> io::Result<()> {
        let spreadsheet = file_api::load(path)?;
        self.spreadsheet = spreadsheet;
        self.history = History::new();
        self.batch.clear();
        self.mark_saved();
        Ok(())
    }

    /// Save the current spreadsheet to disk.
    pub fn save_spreadsheet(&mut self, path: &str) -> io::Result<()> {
        file_api::save(&self.spreadsheet, path)?;
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
            Reference::Single {
                sheet_id,
                row_offset,
                col_offset,
            } => {
                let row = (source_cell.row as i32 + row_offset) as u32;
                let col = (source_cell.col as i32 + col_offset) as u32;
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
            range_start_row_offset,
            range_start_col_offset,
            range_end_row_offset,
            range_end_col_offset,
        }) => {
            let sr = (source_cell.row as i32 + range_start_row_offset) as u32;
            let sc = (source_cell.col as i32 + range_start_col_offset) as u32;
            let er = (source_cell.row as i32 + range_end_row_offset) as u32;
            let ec = (source_cell.col as i32 + range_end_col_offset) as u32;
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

/// Walk the AST and collect all referenced cells as absolute IDs, relative to `cell_id`.
fn find_dependencies_from_relative_references(
    ast: &[Expr],
    cell_id: &AbsoluteCellId,
) -> Vec<AbsoluteCellId> {
    let mut deps = Vec::new();
    for expr in ast {
        if let Expr::Atom(ExprAtom::Reference(r)) = expr {
            match r {
                Reference::Single {
                    sheet_id,
                    row_offset,
                    col_offset,
                } => {
                    deps.push(AbsoluteCellId {
                        sheet_id: *sheet_id,
                        row: (cell_id.row as i32 + row_offset) as u32,
                        col: (cell_id.col as i32 + col_offset) as u32,
                    });
                }
                Reference::Range {
                    sheet_id,
                    range_start_row_offset,
                    range_start_col_offset,
                    range_end_row_offset,
                    range_end_col_offset,
                } => {
                    let start_row = (cell_id.row as i32 + range_start_row_offset) as u32;
                    let start_col = (cell_id.col as i32 + range_start_col_offset) as u32;
                    let end_row = (cell_id.row as i32 + range_end_row_offset) as u32;
                    let end_col = (cell_id.col as i32 + range_end_col_offset) as u32;
                    for row in start_row.min(end_row)..=start_row.max(end_row) {
                        for col in start_col.min(end_col)..=start_col.max(end_col) {
                            deps.push(AbsoluteCellId {
                                sheet_id: *sheet_id,
                                row,
                                col,
                            });
                        }
                    }
                }
            }
        }
    }
    deps
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
