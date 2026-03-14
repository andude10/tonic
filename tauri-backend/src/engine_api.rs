use std::marker::PhantomData;

use fastnum::D256;
use serde::{Deserialize, Serialize};

use std::io;

use crate::file_api;
use crate::parser::{lex_formula, parse_formula, FormulaState};
use crate::storage::grid::{CellContent, CellValue, GridCellId};
use crate::storage::types::{
    AbsoluteCellId, Expr, ExprAtom, Formula, FormulaId, Reference, Spreadsheet,
};

/// A single cell mutation: (cell_id, old_value, new_value).
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

pub struct Engine {
    pub spreadsheet: Spreadsheet,
    history: History,
    batch: Vec<CellUpdate>,
}

// todo: add more comments (explain what happens with dependencies, dependants and the grid)

impl Engine {
    pub fn new() -> Self {
        Self {
            spreadsheet: Spreadsheet::new(),
            history: History::new(),
            batch: Vec::new(),
        }
    }

    pub fn start_batch(&mut self) -> EngineGuard {
        self.batch.clear();
        EngineGuard(PhantomData)
    }

    /// If the cell is defined by a formula, remove this cell from the dependants of its dependencies
    fn remove_cell_from_dependents(&mut self, id: &AbsoluteCellId) {
        let gid = id.grid_cell_id();
        let Some(content) = self.spreadsheet.sheets[id.sheet_id as usize].get_content(&gid) else {
            return;
        };
        let Some(dependencies) = content.dependencies.clone() else {
            return;
        };
        for dep in &dependencies {
            let dep_gid = dep.grid_cell_id();
            self.spreadsheet.sheets[dep.sheet_id as usize].remove_dependant(&dep_gid, id);
        }
    }

    /// If the cell is defined by a formula, add this cell to the dependants of its dependencies
    fn add_cell_to_dependents(&mut self, id: &AbsoluteCellId) {
        let gid = id.grid_cell_id();
        let Some(content) = self.spreadsheet.sheets[id.sheet_id as usize].get_content(&gid) else {
            return;
        };
        let Some(dependencies) = content.dependencies.clone() else {
            return;
        };
        for dep in &dependencies {
            let dep_gid = dep.grid_cell_id();
            self.spreadsheet.sheets[dep.sheet_id as usize].add_dependant(&dep_gid, id);
        }
    }

    /// Parse string and insert appropriate cell.
    pub fn parse_and_insert_string(&mut self, _: &EngineGuard, id: AbsoluteCellId, input: &str) {
        let gid = id.grid_cell_id();
        let old = self.spreadsheet.sheets[id.sheet_id as usize]
            .get_content(&gid)
            .cloned();

        // Remove this cell from dependants of its old dependencies
        self.remove_cell_from_dependents(&id);

        // if not entering formula, just update value
        if !input.starts_with('=') {
            let new = if let Ok(n) = input.parse::<D256>() {
                CellContent::number(n)
            } else {
                CellContent::text(input.to_string())
            };
            self.spreadsheet.sheets[id.sheet_id as usize].insert_value(&gid, new.clone());
            self.batch.push(CellUpdate(id, old, Some(new)));
            return;
        }

        // start parsing formula
        let formula_text = &input[1..];
        let lex_result = lex_formula(formula_text);
        // report any errors during lexing
        if lex_result.has_errors() {
            let new = CellContent::error();
            self.spreadsheet.sheets[id.sheet_id as usize].insert_value(&gid, new.clone());
            self.batch.push(CellUpdate(id, old, Some(new)));
            return;
        }

        // return if empty
        let Some(tokens) = lex_result.into_output() else {
            return;
        };

        let mut state = FormulaState {
            names: &mut self.spreadsheet.names,
            cell_id: gid.clone(),
            expr_arena: Vec::new(),
            dependencies: Vec::new(),
        };
        let (parsed, parse_errs) = parse_formula(&tokens, formula_text.len(), &mut state);
        // report any errors during parsing (syntax, name not found)
        if !parse_errs.is_empty() {
            let new = CellContent::error();
            self.spreadsheet.sheets[id.sheet_id as usize].insert_value(&gid, new.clone());
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
            val: CellValue::Error, // todo: evaluate formula
        };
        self.spreadsheet.sheets[id.sheet_id as usize].insert_value(&gid, new.clone());

        // add this cell as dependant to all its dependencies
        self.add_cell_to_dependents(&id);

        self.batch.push(CellUpdate(id, old, Some(new)));
    }

    pub fn insert_number(&mut self, _: &EngineGuard, id: AbsoluteCellId, n: D256) {
        let gid = id.grid_cell_id();
        let old = self.spreadsheet.sheets[id.sheet_id as usize]
            .get_content(&gid)
            .cloned();
        self.remove_cell_from_dependents(&id);
        let new = CellContent::number(n);
        self.spreadsheet.sheets[id.sheet_id as usize].insert_value(&gid, new.clone());
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
        let gid = id.grid_cell_id();
        let old = self.spreadsheet.sheets[id.sheet_id as usize]
            .get_content(&gid)
            .cloned();
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
            val: CellValue::Error, // todo: evaluate formula
        };
        self.spreadsheet.sheets[id.sheet_id as usize].insert_value(&gid, new.clone());
        self.add_cell_to_dependents(&id);
        self.batch.push(CellUpdate(id, old, Some(new)));
    }

    pub fn delete(&mut self, _: &EngineGuard, id: AbsoluteCellId) {
        let gid = id.grid_cell_id();
        let old = self.spreadsheet.sheets[id.sheet_id as usize]
            .get_content(&gid)
            .cloned();
        self.remove_cell_from_dependents(&id);
        self.spreadsheet.sheets[id.sheet_id as usize].remove_value(&gid);
        self.batch.push(CellUpdate(id, old, None));
    }

    /// Finalize the batch: push to undo log, truncate redo history.
    pub fn end_batch(&mut self, _token: EngineGuard) {
        if self.batch.is_empty() {
            return;
        }

        let changes = std::mem::take(&mut self.batch);
        let bounds = compute_bounds(&changes);

        self.history.log.truncate(self.history.log_position);
        let id = self.history.next_log_id;
        self.history.next_log_id += 1;

        self.history.log.push(HistoryLogEntry {
            id,
            changes,
            bounds,
        });
        self.history.log_position = self.history.log.len();

        // todo!("eval affected cells");
    }

    pub fn undo(&mut self) -> Option<ChangeBounds> {
        if self.history.log_position == 0 {
            return None;
        }
        self.history.log_position -= 1;
        let entry = &self.history.log[self.history.log_position];
        let bounds = entry.bounds.clone();
        let changes: Vec<_> = entry
            .changes
            .iter()
            .map(|CellUpdate(id, old, _)| (id.clone(), old.clone()))
            .collect();
        for (id, old) in changes {
            self.remove_cell_from_dependents(&id);
            let gid = id.grid_cell_id();
            match old {
                Some(content) => {
                    self.spreadsheet.sheets[id.sheet_id as usize].insert_value(&gid, content);
                    self.add_cell_to_dependents(&id);
                }
                None => self.spreadsheet.sheets[id.sheet_id as usize].remove_value(&gid),
            }
        }
        // todo!("eval affected cells");
        Some(bounds)
    }

    pub fn redo(&mut self) -> Option<ChangeBounds> {
        if self.history.log_position >= self.history.log.len() {
            return None;
        }
        let entry = &self.history.log[self.history.log_position];
        let bounds = entry.bounds.clone();
        let changes: Vec<_> = entry
            .changes
            .iter()
            .map(|CellUpdate(id, _, new)| (id.clone(), new.clone()))
            .collect();
        self.history.log_position += 1;
        for (id, new) in changes {
            self.remove_cell_from_dependents(&id);
            let gid = id.grid_cell_id();
            match new {
                Some(content) => {
                    self.spreadsheet.sheets[id.sheet_id as usize].insert_value(&gid, content);
                    self.add_cell_to_dependents(&id);
                }
                None => self.spreadsheet.sheets[id.sheet_id as usize].remove_value(&gid),
            }
        }
        // todo!("eval affected cells");
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

/// Walk the AST and collect all referenced cells as absolute IDs, resolved relative to `cell_id`.
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
