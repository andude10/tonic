use std::collections::BTreeMap;

use rust_decimal::Decimal;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Serde helper: serialize `Vec<BTreeMap<K, V>>` as `Vec<Vec<(K, V)>>`.
/// Avoids the JSON requirement that map keys must be strings.
mod vec_btreemap_as_vec {
    use super::*;

    pub fn serialize<S, K, V>(maps: &Vec<BTreeMap<K, V>>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        K: Serialize + Ord,
        V: Serialize,
    {
        let vecs: Vec<Vec<(&K, &V)>> = maps.iter().map(|m| m.iter().collect()).collect();
        vecs.serialize(serializer)
    }

    pub fn deserialize<'de, D, K, V>(deserializer: D) -> Result<Vec<BTreeMap<K, V>>, D::Error>
    where
        D: Deserializer<'de>,
        K: Deserialize<'de> + Ord,
        V: Deserialize<'de>,
    {
        let vecs: Vec<Vec<(K, V)>> = Vec::deserialize(deserializer)?;
        Ok(vecs.into_iter().map(|v| v.into_iter().collect()).collect())
    }
}

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
pub type ProjectionId = u32;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Table {
    pub sheet_id: SheetId,
    pub name: String,
    pub first_header: GridCellId,
    pub last_header: GridCellId,
    pub body_start: GridCellId,
    pub body_end: GridCellId,
    pub projection_id: ProjectionId,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ProjectionFilterOption {
    pub id: u32,
    pub selected: bool,
    pub count: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ProjectionSortOption {
    pub selected: bool,
    pub desc: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Projection {
    pub sheet_id: SheetId,
    pub projection_start: GridCellId,
    pub projection_end: GridCellId,
    pub projected_rows: Vec<u32>,
    pub active: bool,
    #[serde(with = "vec_btreemap_as_vec")]
    pub filter_options_per_column: Vec<BTreeMap<CellValue, ProjectionFilterOption>>,
    pub sorting_options_per_column: Vec<ProjectionSortOption>,
    pub hidden_rows_count: u32,
    pub filter_show_blanks: Vec<bool>,
    pub next_filter_option_id: u32,
}

pub type Sheets = Vec<Grid>;

#[derive(Serialize, Deserialize)]
pub struct Spreadsheet {
    // todo: sheets are private for engine, should not be used in lib.rs
    pub(crate) sheets: Sheets,
    pub(crate) formulas: StableVec<Formula>,
    pub(crate) names: SpreadsheetNames,
    pub(crate) tables: StableVec<Table>,
    pub(crate) projections: StableVec<Projection>,
}

impl Spreadsheet {
    pub fn new() -> Self {
        Self {
            sheets: vec![Grid::default()],
            formulas: StableVec::new(),
            names: SpreadsheetNames::new(),
            tables: StableVec::new(),
            projections: StableVec::new(),
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

    /// Find the table whose body contains `cell`, returning (table_id, &Table).
    pub fn find_table_containing_cell(&self, cell: &AbsoluteCellId) -> Option<(u32, &Table)> {
        self.tables
            .iter()
            .enumerate()
            .filter_map(|(id, t)| t.as_ref().map(|t| (id as u32, t)))
            .find(|(_, t)| {
                t.sheet_id == cell.sheet_id
                    && cell.row >= t.body_start.row
                    && cell.row <= t.body_end.row
                    && cell.col >= t.body_start.col
                    && cell.col <= t.body_end.col
            })
    }

    pub fn get_projected_content(&self, id: &AbsoluteCellId) -> Option<&CellContent> {
        if let Some((_, table)) = self.find_table_containing_cell(id) {
            let projection = self.projections.get(table.projection_id).unwrap();
            if projection.active {
                let visual_idx = (id.row - projection.projection_start.row) as usize;
                if visual_idx < projection.projected_rows.len() {
                    return self.get_content(&AbsoluteCellId {
                        sheet_id: id.sheet_id,
                        row: projection.projected_rows[visual_idx],
                        col: id.col,
                    });
                }
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
