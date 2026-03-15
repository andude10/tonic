use fastnum::D256;
use serde::{Deserialize, Serialize};

use crate::storage::{
    grid::{Grid, GridCellId},
    name_resolution::SpreadsheetNames,
    stable_vec::StableVec,
};

pub type SheetId = u32;
pub type FormulaId = u32;
pub type UserFuncId = u32;
pub type ExprId = u32;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub struct AbsoluteCellId {
    pub sheet_id: SheetId,
    pub row: u32,
    pub col: u32,
}

impl AbsoluteCellId {
    // todo: remove
    pub fn grid_cell_id(&self) -> GridCellId {
        GridCellId {
            row: self.row,
            col: self.col,
        }
    }
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
    Reference,
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

    // todo:
    ExtrnalFunctionCall {
        func_id: UserFuncId,
        args: Vec<ExprId>,
    },
}

/// Repressentation of formula. If multiple cells contain the same FormulaId, then they share single formula
///
/// When new formula is entered by user, it is parsed and preserved. During parsing, all references
/// are converted into R1C1 format (meaning AST stores relative offsets, instead of exact IDs of cells) .
/// If the formula is cloned, then it becomes shared formula (multiple cells will contain same FormulaId)
#[derive(Serialize, Deserialize)]
pub struct Formula {
    /// Abstract syntax tree (result of parsing `formula_string`)
    pub ast: Vec<Expr>,

    /// Original text, entered by user to create this formula (preserves spaces)
    ///
    /// References should be adjusted relative to offsets if the formula is shared.
    pub formula_string: String,
}

pub type Sheets = Vec<Grid>;

#[derive(Serialize, Deserialize)]
pub struct Spreadsheet {
    pub(crate) sheets: Sheets,
    pub(crate) formulas: StableVec<Formula>,
    pub(crate) names: SpreadsheetNames,
    // todo:
    // pub user_functions: Vec<UserFunction>,
}

impl Spreadsheet {
    pub fn new() -> Self {
        Self {
            sheets: vec![Grid::new(12_500, 2)],
            formulas: StableVec::new(),
            names: SpreadsheetNames::new(),
        }
    }
}
