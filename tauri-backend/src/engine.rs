use fastnum::D256;

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
            ExprAtom::RelativeCellRef(_, _) => AtomType::RelativeCellRef,
            ExprAtom::CellRange(_, _) => AtomType::CellRange,
            ExprAtom::RelativeCellRange(_, _) => AtomType::RelativeCellRange,
        }
    }

    fn as_number(&self, spreadsheet: &Spreadsheet) -> Result<D256, EvalError> {
        match self {
            ExprAtom::Number(n) => Ok(*n),
            ExprAtom::CellRef(sheet_id, cell_id) | ExprAtom::RelativeCellRef(sheet_id, cell_id) => {
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
            ExprAtom::CellRange(sheet_id, range) | ExprAtom::RelativeCellRange(sheet_id, range) => {
                Ok((*sheet_id, *range))
            }
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
) -> Result<CellValue, EvalError> {
    let mut store: Vec<ExprAtom> = Vec::with_capacity(formula_exprs.len());

    for expr in formula_exprs {
        let res = match expr {
            Expr::Atom(atom) => atom.clone(),
            Expr::Negate(id) => {
                let n = store[*id as usize].as_number(spreadsheet)?;
                ExprAtom::Number(-n)
            }
            Expr::Add(a_id, b_id) => {
                let a = store[*a_id as usize].as_number(spreadsheet)?;
                let b = store[*b_id as usize].as_number(spreadsheet)?;
                ExprAtom::Number(a + b)
            }
            Expr::Subtract(a_id, b_id) => {
                let a = store[*a_id as usize].as_number(spreadsheet)?;
                let b = store[*b_id as usize].as_number(spreadsheet)?;
                ExprAtom::Number(a - b)
            }
            Expr::Multiply(a_id, b_id) => {
                let a = store[*a_id as usize].as_number(spreadsheet)?;
                let b = store[*b_id as usize].as_number(spreadsheet)?;
                ExprAtom::Number(a * b)
            }
            Expr::Divide(a_id, b_id) => {
                let a = store[*a_id as usize].as_number(spreadsheet)?;
                let b = store[*b_id as usize].as_number(spreadsheet)?;
                ExprAtom::Number(a / b)
            }
            Expr::Sum { range_id, mut sum } => {
                let (sheet_id, start, end) = store[*range_id as usize].as_range_normalized()?;
                for (_, cell) in spreadsheet.sheets[sheet_id as usize].range(start..=end) {
                    // todo: remove branching?
                    if let Cell::SingleValue(CellValue::Number(n)) = cell {
                        sum += *n;
                    }
                    if let Cell::Formula { value, .. } = cell {
                        if let CellValue::Number(n) = value {
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
                let (sheet_id, start, end) = store[*range_id as usize].as_range_normalized()?;
                for (_, cell) in spreadsheet.sheets[sheet_id as usize].range(start..=end) {
                    count += 1;
                    if let Cell::SingleValue(CellValue::Number(n)) = cell {
                        sum += *n;
                    }
                    if let Cell::Formula { value, .. } = cell {
                        if let CellValue::Number(n) = value {
                            sum += *n;
                        }
                    }
                }
                ExprAtom::Number(sum / count as f64)
            }
            Expr::ExtrnalFunctionCall { .. } => todo!(),
        };
        store.push(res);
    }

    let value = match store.pop() {
        Some(ExprAtom::Number(n)) => CellValue::Number(n),
        Some(ExprAtom::Text(s)) => CellValue::Text(s),
        Some(ExprAtom::Boolean(b)) => CellValue::Text(b.to_string()),
        Some(other) => CellValue::Text(format!("{:?}", other)),
        None => CellValue::Text(String::new()),
    };

    Ok(value)
}

// Expr::Atom(expr_atom) => match expr_atom {
//     ExprAtom::Boolean(x) => CellValue::Text(x.to_string()),
//     ExprAtom::Number(x) => CellValue::Number(*x),
//     ExprAtom::Text(x) => CellValue::Text(x.clone()),
//     ExprAtom::Function(x) => {
//         let name = spreadsheet
//             .user_function_names_lookup
//             .get(x)
//             .map(|nr| nr.to_string())
//             .unwrap_or_default();
//         CellValue::Text(name)
//     }
//     ExprAtom::CellRef(opt_sheet_id, cell_id) => spreadsheet
//         .get_cell_value(0, opt_sheet_id, cell_id)
//         .cloned()
//         .unwrap_or(CellValue::Number(0.0)),
//     ExprAtom::RelativeCellRef(opt_sheet_id, cell_id) => spreadsheet
//         .get_cell_value(0, opt_sheet_id, cell_id)
//         .cloned()
//         .unwrap_or(CellValue::Number(0.0)),
//     ExprAtom::CellRange(opt_sheet_id, cell_range) => todo!(),
//     ExprAtom::RelativeCellRange(opt_sheet_id, cell_range) => todo!(),
// },
