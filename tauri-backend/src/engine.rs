use std::collections::{HashMap, HashSet};

use fastnum::D256;
use tauri_plugin_log::log::debug;

use crate::sheet::{AtomType, Cell, CellId, CellRange, CellValue, Expr, ExprAtom, Spreadsheet};

#[derive(Debug)]
pub enum EvalError {
    TypeError { expected: AtomType, got: AtomType },
}

impl ExprAtom {
    fn atom_type(&self) -> AtomType {
        match self {
            ExprAtom::Boolean(_) => AtomType::Boolean,
            ExprAtom::Number(_) => AtomType::Number,
            ExprAtom::Text(_) => AtomType::Text,
            ExprAtom::Function(_) => AtomType::Function,
            ExprAtom::CellRef(_, _) => AtomType::CellRef,
            ExprAtom::CellRange(_, _) => AtomType::CellRange,
        }
    }

    fn as_number(&self, spreadsheet: &Spreadsheet) -> Result<D256, EvalError> {
        match self {
            ExprAtom::Number(n) => Ok(*n),
            ExprAtom::CellRef(sheet_id, cell_id) => {
                match spreadsheet.get_cell_value(cell_id, *sheet_id) {
                    Some(CellValue::Number(n)) => Ok(*n),
                    _ => Ok(D256::ZERO),
                }
            }
            other => Err(EvalError::TypeError {
                expected: AtomType::Number,
                got: other.atom_type(),
            }),
        }
    }

    fn as_range(&self) -> Result<(u32, CellRange), EvalError> {
        match self {
            ExprAtom::CellRange(sheet_id, range) => Ok((*sheet_id, *range)),
            other => Err(EvalError::TypeError {
                expected: AtomType::CellRange,
                got: other.atom_type(),
            }),
        }
    }

    fn as_range_normalized(&self) -> Result<(u32, CellId, CellId), EvalError> {
        let (sheet_id, range) = self.as_range()?;
        let lo = CellId {
            col: range.start.col.min(range.end.col),
            row: range.start.row.min(range.end.row),
        };
        let hi = CellId {
            col: range.start.col.max(range.end.col),
            row: range.start.row.max(range.end.row),
        };
        Ok((sheet_id, lo, hi))
    }
}

// todo: figure out how to reference cell (remove sheet_id?)
pub fn eval_formula(
    formula_exprs: &[Expr],
    spreadsheet: &Spreadsheet,
    eval_store: &mut Vec<ExprAtom>,
) -> Result<CellValue, EvalError> {
    eval_store.clear();

    for expr in formula_exprs {
        let res = match expr {
            Expr::Atom(atom) => atom.clone(),
            Expr::Negate(id) => {
                let n = eval_store[*id as usize].as_number(spreadsheet)?;
                ExprAtom::Number(-n)
            }
            Expr::Add(a_id, b_id) => {
                let a = eval_store[*a_id as usize].as_number(spreadsheet)?;
                let b = eval_store[*b_id as usize].as_number(spreadsheet)?;
                ExprAtom::Number(a + b)
            }
            Expr::Subtract(a_id, b_id) => {
                let a = eval_store[*a_id as usize].as_number(spreadsheet)?;
                let b = eval_store[*b_id as usize].as_number(spreadsheet)?;
                ExprAtom::Number(a - b)
            }
            Expr::Multiply(a_id, b_id) => {
                let a = eval_store[*a_id as usize].as_number(spreadsheet)?;
                let b = eval_store[*b_id as usize].as_number(spreadsheet)?;
                ExprAtom::Number(a * b)
            }
            Expr::Divide(a_id, b_id) => {
                let a = eval_store[*a_id as usize].as_number(spreadsheet)?;
                let b = eval_store[*b_id as usize].as_number(spreadsheet)?;
                ExprAtom::Number(a / b)
            }
            Expr::Sum { range_id, mut sum } => {
                let (sheet_id, start, end) =
                    eval_store[*range_id as usize].as_range_normalized()?;
                let sheet = &spreadsheet.sheets[sheet_id as usize].btree;
                // todo: remove branching?
                for col in start.col..=end.col {
                    let col_start = CellId {
                        col,
                        row: start.row,
                    };
                    let col_end = CellId { col, row: end.row };
                    for (_, cell) in sheet.range(col_start..=col_end) {
                        if let Cell::SingleValue(CellValue::Number(n)) = cell {
                            sum += *n;
                        }
                        if let Cell::Formula {
                            value: Some(CellValue::Number(n)),
                            ..
                        } = cell
                        {
                            sum += *n;
                        }
                    }
                }
                ExprAtom::Number(sum)
            }
            Expr::Avg {
                range_id,
                mut sum,
                mut count,
            } => {
                let (sheet_id, start, end) =
                    eval_store[*range_id as usize].as_range_normalized()?;
                let sheet = &spreadsheet.sheets[sheet_id as usize].btree;
                for col in start.col..=end.col {
                    let col_start = CellId {
                        col,
                        row: start.row,
                    };
                    let col_end = CellId { col, row: end.row };
                    for (_, cell) in sheet.range(col_start..=col_end) {
                        count += 1;
                        if let Cell::SingleValue(CellValue::Number(n)) = cell {
                            sum += *n;
                        }
                        if let Cell::Formula {
                            value: Some(CellValue::Number(n)),
                            ..
                        } = cell
                        {
                            sum += *n;
                        }
                    }
                }
                ExprAtom::Number(sum / count as f64)
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

/// Recalculate all formulas affected by modified cells, propagating in waves.
pub fn eval(modified_cells: &[CellId], spreadsheet: &mut Spreadsheet) {
    let sheet = &spreadsheet.sheets[0];

    // 1. Build affected set: modified cells + their direct dependents
    let mut affected: HashSet<CellId> = HashSet::new();
    for &cell_id in modified_cells {
        affected.insert(cell_id);
        if let Some(deps) = sheet.dependents.get(&cell_id) {
            for &dep in deps {
                affected.insert(dep);
            }
        }
    }

    // 2. Compute active_dependency_count: for each affected cell, count how many
    //    of its dependencies are also in the affected set
    let mut active_dep_count: HashMap<CellId, u32> = HashMap::new();
    for &cell_id in &affected {
        active_dep_count.insert(
            cell_id,
            compute_active_dep_count(cell_id, &affected, spreadsheet),
        );
    }

    let mut eval_store: Vec<ExprAtom> = Vec::new();

    // 3. Wave loop: evaluate cells with no pending dependencies, propagate changes
    loop {
        let wave: Vec<CellId> = affected
            .iter()
            .filter(|c| active_dep_count.get(c).copied().unwrap_or(0) == 0)
            .copied()
            .collect();

        if wave.is_empty() {
            break;
        }

        for &cell_id in &wave {
            affected.remove(&cell_id);

            // evaluate formula cells (plain values are already updated by caller)
            if let Some(Cell::Formula {
                expr,
                value: prev_value,
                ..
            }) = spreadsheet.sheets[0].btree.get(&cell_id)
            {
                let expr = expr.clone();
                let prev_value = prev_value.clone();
                let new_value = match eval_formula(&expr, spreadsheet, &mut eval_store) {
                    Ok(v) => v,
                    Err(e) => CellValue::FormulaError(format!("Eval Error: {:?}", e)),
                };
                let value_changed = match &prev_value {
                    Some(pv) => !cell_values_equal(pv, &new_value),
                    None => true,
                };

                spreadsheet.sheets[0].btree.insert(
                    cell_id,
                    Cell::Formula {
                        expr,
                        value: Some(new_value),
                        prev_value,
                    },
                );

                // if value changed, add dependents into affected set
                if value_changed {
                    if let Some(deps) = spreadsheet.sheets[0].dependents.get(&cell_id) {
                        for &dep in deps {
                            if !affected.contains(&dep) {
                                affected.insert(dep);
                                active_dep_count.insert(
                                    dep,
                                    compute_active_dep_count(dep, &affected, spreadsheet),
                                );
                            }
                        }
                    }
                }
            }

            // decrement active_dep_count for dependents still in the affected set
            if let Some(deps) = spreadsheet.sheets[0].dependents.get(&cell_id) {
                for &dep in deps {
                    if let Some(count) = active_dep_count.get_mut(&dep) {
                        *count = count.saturating_sub(1);
                    }
                }
            }
        }
    }
}

fn compute_active_dep_count(
    cell_id: CellId,
    affected: &HashSet<CellId>,
    spreadsheet: &Spreadsheet,
) -> u32 {
    let Some(deps) = spreadsheet.sheets[0].dependencies.get(&cell_id) else {
        return 0;
    };
    deps.iter()
        .map(|dep| {
            let mut c = 0u32;
            dep.for_each_cell(|ref_cell| {
                if affected.contains(&ref_cell) {
                    c += 1;
                }
            });
            c
        })
        .sum()
}

fn cell_values_equal(a: &CellValue, b: &CellValue) -> bool {
    match (a, b) {
        (CellValue::Number(a), CellValue::Number(b)) => a == b,
        (CellValue::Text(a), CellValue::Text(b)) => a == b,
        (CellValue::FormulaError(a), CellValue::FormulaError(b)) => a == b,
        _ => false,
    }
}
