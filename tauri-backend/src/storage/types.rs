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

pub type TableId = u32;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Table {
    pub sheet_id: SheetId,
    pub first_header: GridCellId,
    pub last_header: GridCellId,
    pub body_start: GridCellId,
    pub body_end: GridCellId,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum Projection {
    Sort {
        sheet_id: SheetId,
        projection_start: GridCellId,
        projection_end: GridCellId,
        sorted_rows: Vec<u32>,
    },
    Filter {
        sheet_id: SheetId,
        projection_start: GridCellId,
        projection_end: GridCellId,
        matched_rows: Vec<u32>,
        hidden_rows_count: u32,
    },
}

pub type Sheets = Vec<Grid>;

#[derive(Serialize, Deserialize)]
pub struct Spreadsheet {
    pub(crate) sheets: Sheets,
    pub(crate) formulas: StableVec<Formula>,
    pub(crate) names: SpreadsheetNames,
    #[serde(default)]
    pub(crate) tables: StableVec<Table>,
    #[serde(default, skip)]
    pub(crate) projections: Vec<Projection>,
}

impl Spreadsheet {
    pub fn new() -> Self {
        Self {
            sheets: vec![Grid::new(12_500, 2)],
            formulas: StableVec::new(),
            names: SpreadsheetNames::new(),
            tables: StableVec::new(),
            projections: Vec::new(),
        }
    }

    /// Find the table whose header row contains `header`, returning (table_id, &Table).
    pub fn find_table_by_header(&self, header: &GridCellId) -> Option<(u32, &Table)> {
        self.tables
            .iter()
            .enumerate()
            .filter_map(|(id, t)| t.as_ref().map(|t| (id as u32, t)))
            .find(|(_, t)| {
                t.first_header.row == header.row
                    && header.col >= t.first_header.col
                    && header.col <= t.last_header.col
            })
    }

    /// Find the index of a Sort projection whose bounds match the given table body.
    pub fn find_sort_projection_for_table(&self, table: &Table) -> Option<usize> {
        self.projections.iter().position(|p| {
            matches!(p, Projection::Sort { projection_start, projection_end, .. }
                if *projection_start == table.body_start && *projection_end == table.body_end)
        })
    }

    pub fn get_projected_content(&self, id: &AbsoluteCellId) -> Option<&CellContent> {
        for projection in &self.projections {
            match projection {
                Projection::Sort {
                    sheet_id,
                    projection_start,
                    projection_end,
                    sorted_rows,
                } => {
                    if id.sheet_id == *sheet_id
                        && id.row >= projection_start.row
                        && id.row <= projection_end.row
                        && id.col >= projection_start.col
                        && id.col <= projection_end.col
                    {
                        let visual_idx = (id.row - projection_start.row) as usize;
                        if visual_idx < sorted_rows.len() {
                            let actual_id = AbsoluteCellId {
                                sheet_id: id.sheet_id,
                                row: sorted_rows[visual_idx],
                                col: id.col,
                            };
                            return self.get_content(&actual_id);
                        }
                    }
                }
                Projection::Filter { .. } => continue,
            }
        }
        self.get_content(id)
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
