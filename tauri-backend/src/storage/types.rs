use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::storage::{
    grid::{CellContent, CellValue, Grid, GridCellId},
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

impl From<&AbsoluteCellId> for GridCellId {
    fn from(id: &AbsoluteCellId) -> Self {
        GridCellId {
            row: id.row,
            col: id.col,
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
    Number(Decimal),
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

    // todo: remove this mess.

    pub fn get_content(&self, id: &AbsoluteCellId) -> Option<&CellContent> {
        self.sheets[id.sheet_id as usize].get_content(&id.into())
    }

    pub fn get_value(&self, id: &AbsoluteCellId) -> Option<&CellValue> {
        self.sheets[id.sheet_id as usize].get_value(&id.into())
    }

    pub fn set_value(&mut self, id: &AbsoluteCellId, val: CellValue) {
        self.sheets[id.sheet_id as usize].set_value(&id.into(), val);
    }

    pub fn insert_content(&mut self, id: &AbsoluteCellId, content: CellContent) {
        self.sheets[id.sheet_id as usize].insert_content(&id.into(), content);
    }

    pub fn remove_content(&mut self, id: &AbsoluteCellId) {
        self.sheets[id.sheet_id as usize].remove_content(&id.into());
    }

    pub fn get_dependents(&self, id: &AbsoluteCellId) -> Option<&Vec<AbsoluteCellId>> {
        self.sheets[id.sheet_id as usize].get_dependents(&id.into())
    }

    pub fn add_dependant(&mut self, id: &AbsoluteCellId, dependant: &AbsoluteCellId) {
        self.sheets[id.sheet_id as usize].add_dependant(&id.into(), dependant);
    }

    pub fn remove_dependant(&mut self, id: &AbsoluteCellId, dependant: &AbsoluteCellId) {
        self.sheets[id.sheet_id as usize].remove_dependant(&id.into(), dependant);
    }

    pub fn increase_pending_dependencies(&mut self, id: &AbsoluteCellId) {
        self.sheets[id.sheet_id as usize].increase_pending_dependencies(&id.into());
    }

    pub fn decrease_pending_dependencies(&mut self, id: &AbsoluteCellId) {
        self.sheets[id.sheet_id as usize].decrease_pending_dependencies(&id.into());
    }

    pub fn get_pending_dependencies(&self, id: &AbsoluteCellId) -> u32 {
        self.sheets[id.sheet_id as usize].get_pending_dependencies(&id.into())
    }
}
