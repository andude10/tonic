use std::collections::HashMap;

use fastnum::D256;
use serde::{Deserialize, Serialize};

use crate::storage::{
    grid::{Grid, GridCellId},
    stable_vec::StableVec,
};

pub type SheetId = u32;
pub type FormulaId = u32;
pub type UserFuncId = u32;
pub type ExprId = u32;

#[derive(Serialize, Deserialize, Debug, Clone, Hash, PartialEq, Eq)]
pub struct AbsoluteCellId {
    sheet_id: SheetId,
    row: u32,
    col: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum Reference {
    Single {
        sheet_id: SheetId,
        row_offset: i32,
        col_offset: i32,
    },
    Range {
        sheet_id: SheetId,
        range_start_row_offset: i32,
        range_start_col_offset: i32,
        range_end_row_offset: i32,
        range_end_col_offset: i32,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum ExprAtom {
    Boolean(bool),
    Number(D256),
    Text(String),
    Function(UserFuncId),
    Reference(Reference),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum AtomType {
    Boolean,
    Number,
    Text,
    Function,
    CellRef,
    CellRange,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum Expr {
    Atom(ExprAtom),
    Negate(ExprId),
    Add(ExprId, ExprId),
    Subtract(ExprId, ExprId),
    Multiply(ExprId, ExprId),
    Divide(ExprId, ExprId),

    // default formulas
    Sum,
    Avg,

    ExtrnalFunctionCall {
        func_id: UserFuncId,
        args: Vec<ExprId>,
    },
}

pub struct Formula {
    exprs: Vec<Expr>,
    applied_cells: Vec<Reference>,
}

pub type Sheets = Vec<Grid>;

pub struct Spreadsheet {
    sheets: Sheets,
    formulas: StableVec<Formula>,
    user_strings: HashMap<AbsoluteCellId, String>,
    // todo:
    // pub user_functions: Vec<UserFunction>,
}
