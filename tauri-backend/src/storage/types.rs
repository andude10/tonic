use std::collections::BTreeMap;
use std::fmt;

use rust_decimal::Decimal;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Serde helper: serialize `Vec<BTreeMap<K, V>>` as entry lists.
/// Avoids the JSON requirement that map keys must be strings.
mod vec_btreemap_as_vec {
    use super::*;

    #[derive(Serialize, Deserialize)]
    struct Entry<K, V> {
        key: K,
        value: V,
    }

    pub fn serialize<S, K, V>(maps: &Vec<BTreeMap<K, V>>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        K: Serialize + Ord,
        V: Serialize,
    {
        let vecs: Vec<Vec<Entry<&K, &V>>> = maps
            .iter()
            .map(|m| m.iter().map(|(key, value)| Entry { key, value }).collect())
            .collect();
        vecs.serialize(serializer)
    }

    pub fn deserialize<'de, D, K, V>(deserializer: D) -> Result<Vec<BTreeMap<K, V>>, D::Error>
    where
        D: Deserializer<'de>,
        K: Deserialize<'de> + Ord,
        V: Deserialize<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Maps<K, V> {
            Entries(Vec<Vec<Entry<K, V>>>),
            Tuples(Vec<Vec<(K, V)>>),
        }

        match Maps::deserialize(deserializer)? {
            Maps::Entries(vecs) => Ok(vecs
                .into_iter()
                .map(|v| {
                    v.into_iter()
                        .map(|entry| (entry.key, entry.value))
                        .collect()
                })
                .collect()),
            Maps::Tuples(vecs) => Ok(vecs.into_iter().map(|v| v.into_iter().collect()).collect()),
        }
    }
}

use std::sync::Arc;

use parking_lot::RwLock;

use crate::storage::{
    cell_properties::CellProperties,
    dependency_graph::DependencyGraph,
    grid::{Cell, CellValue, Grid, GridCellId},
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

#[derive(Serialize, Deserialize, Debug, Clone, Copy, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct CellRange {
    pub sheet_id: SheetId,
    pub start_row: u32,
    pub start_col: u32,
    pub end_row: u32,
    pub end_col: u32,
}

impl From<&AbsoluteCellId> for GridCellId {
    fn from(id: &AbsoluteCellId) -> Self {
        GridCellId {
            row: id.row,
            col: id.col,
        }
    }
}

impl CellRange {
    pub fn new(
        sheet_id: SheetId,
        start_row: u32,
        start_col: u32,
        end_row: u32,
        end_col: u32,
    ) -> Self {
        Self {
            sheet_id,
            start_row: start_row.min(end_row),
            start_col: start_col.min(end_col),
            end_row: start_row.max(end_row),
            end_col: start_col.max(end_col),
        }
    }

    pub fn single(id: AbsoluteCellId) -> Self {
        Self {
            sheet_id: id.sheet_id,
            start_row: id.row,
            start_col: id.col,
            end_row: id.row,
            end_col: id.col,
        }
    }

    pub fn head_cell(&self) -> AbsoluteCellId {
        AbsoluteCellId {
            sheet_id: self.sheet_id,
            row: self.start_row,
            col: self.start_col,
        }
    }

    pub fn tail_cell(&self) -> AbsoluteCellId {
        AbsoluteCellId {
            sheet_id: self.sheet_id,
            row: self.end_row,
            col: self.end_col,
        }
    }

    pub fn is_single(&self) -> bool {
        self.start_row == self.end_row && self.start_col == self.end_col
    }

    pub fn is_row_vector(&self) -> bool {
        self.start_row == self.end_row
    }

    pub fn is_col_vector(&self) -> bool {
        self.start_col == self.end_col
    }

    pub fn is_line(&self) -> bool {
        self.is_row_vector() || self.is_col_vector()
    }

    pub fn cell_count(&self) -> u32 {
        (self.end_row - self.start_row + 1) * (self.end_col - self.start_col + 1)
    }

    pub fn for_each_cell<F>(&self, mut f: F)
    where
        F: FnMut(AbsoluteCellId),
    {
        for row in self.start_row..=self.end_row {
            for col in self.start_col..=self.end_col {
                f(AbsoluteCellId {
                    sheet_id: self.sheet_id,
                    row,
                    col,
                });
            }
        }
    }

    pub fn intersects(&self, other: &Self) -> bool {
        self.sheet_id == other.sheet_id
            && self.start_row <= other.end_row
            && other.start_row <= self.end_row
            && self.start_col <= other.end_col
            && other.start_col <= self.end_col
    }

    pub fn intersection(&self, other: &Self) -> Option<Self> {
        if !self.intersects(other) {
            return None;
        }
        Some(Self {
            sheet_id: self.sheet_id,
            start_row: self.start_row.max(other.start_row),
            start_col: self.start_col.max(other.start_col),
            end_row: self.end_row.min(other.end_row),
            end_col: self.end_col.min(other.end_col),
        })
    }

    pub fn contains(&self, other: &Self) -> bool {
        self.sheet_id == other.sheet_id
            && self.start_row <= other.start_row
            && self.start_col <= other.start_col
            && self.end_row >= other.end_row
            && self.end_col >= other.end_col
    }

    pub fn bounding_union(&self, other: &Self) -> Self {
        Self {
            sheet_id: self.sheet_id,
            start_row: self.start_row.min(other.start_row),
            start_col: self.start_col.min(other.start_col),
            end_row: self.end_row.max(other.end_row),
            end_col: self.end_col.max(other.end_col),
        }
    }

    pub fn shifted(&self, row_delta: i32, col_delta: i32) -> Option<Self> {
        fn apply_delta(value: u32, delta: i32) -> Option<u32> {
            let shifted = value as i64 + delta as i64;
            (shifted >= 0).then_some(shifted as u32)
        }

        Some(Self {
            sheet_id: self.sheet_id,
            start_row: apply_delta(self.start_row, row_delta)?,
            start_col: apply_delta(self.start_col, col_delta)?,
            end_row: apply_delta(self.end_row, row_delta)?,
            end_col: apply_delta(self.end_col, col_delta)?,
        })
    }

    pub fn subtract(&self, other: &Self) -> Vec<Self> {
        let Some(overlap) = self.intersection(other) else {
            return vec![*self];
        };
        if overlap == *self {
            return Vec::new();
        }

        let mut result = Vec::new();
        if self.start_row < overlap.start_row {
            result.push(Self::new(
                self.sheet_id,
                self.start_row,
                self.start_col,
                overlap.start_row - 1,
                self.end_col,
            ));
        }

        if overlap.end_row < self.end_row {
            result.push(Self::new(
                self.sheet_id,
                overlap.end_row + 1,
                self.start_col,
                self.end_row,
                self.end_col,
            ));
        }

        if self.start_col < overlap.start_col {
            result.push(Self::new(
                self.sheet_id,
                overlap.start_row,
                self.start_col,
                overlap.end_row,
                overlap.start_col - 1,
            ));
        }

        if overlap.end_col < self.end_col {
            result.push(Self::new(
                self.sheet_id,
                overlap.start_row,
                overlap.end_col + 1,
                overlap.end_row,
                self.end_col,
            ));
        }

        result
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Coordinate {
    Absolute(u32),
    Relative(i32),
}

impl Coordinate {
    pub fn to_index(&self, base: u32) -> u32 {
        match self {
            Coordinate::Absolute(index) => *index,
            Coordinate::Relative(offset) => (base as i32 + offset).max(0) as u32,
        }
    }

    // less than equals
    pub fn leq(&self, other: u32) -> bool {
        match self {
            Coordinate::Absolute(a) => *a <= other,
            Coordinate::Relative(a) => *a <= other as i32,
        }
    }

    // less than
    pub fn lt(&self, other: u32) -> bool {
        match self {
            Coordinate::Absolute(a) => *a < other,
            Coordinate::Relative(a) => *a < other as i32,
        }
    }

    pub fn increase(&self) -> Coordinate {
        match self {
            Coordinate::Absolute(index) => Coordinate::Absolute(index + 1),
            Coordinate::Relative(offset) => Coordinate::Relative(offset + 1),
        }
    }

    pub fn decrease(&self) -> Coordinate {
        match self {
            Coordinate::Absolute(index) => Coordinate::Absolute((index - 1).max(0)),
            Coordinate::Relative(offset) => Coordinate::Relative((offset - 1).max(0)),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
pub enum Reference {
    Single {
        sheet_id: SheetId,
        row: Coordinate,
        col: Coordinate,
    },
    Range {
        sheet_id: SheetId,
        start_row: Coordinate,
        start_col: Coordinate,
        end_row: Coordinate,
        end_col: Coordinate,
    },
}

impl Reference {
    pub fn to_cell_range(&self, source_cell: &AbsoluteCellId) -> CellRange {
        match self {
            Reference::Single { sheet_id, row, col } => CellRange::new(
                *sheet_id,
                row.to_index(source_cell.row),
                col.to_index(source_cell.col),
                row.to_index(source_cell.row),
                col.to_index(source_cell.col),
            ),
            Reference::Range {
                sheet_id,
                start_row,
                start_col,
                end_row,
                end_col,
            } => CellRange::new(
                *sheet_id,
                start_row.to_index(source_cell.row),
                start_col.to_index(source_cell.col),
                end_row.to_index(source_cell.row),
                end_col.to_index(source_cell.col),
            ),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum ExprAtom {
    Bool(bool),
    Number(Decimal),
    Text(String),

    // todo: rename to Error?
    InvalidReferenceError(String),

    Function(UserFuncId),
    Reference(Reference),
}

impl From<CellValue> for ExprAtom {
    fn from(val: CellValue) -> Self {
        match val {
            CellValue::Number(n) => ExprAtom::Number(n),
            CellValue::Text(s) => ExprAtom::Text(s.to_string()),
            CellValue::Bool(b) => ExprAtom::Bool(b),
            CellValue::Error(s, _) => ExprAtom::InvalidReferenceError(s.to_string()),
        }
    }
}

impl From<ExprAtom> for CellValue {
    fn from(atom: ExprAtom) -> Self {
        match atom {
            ExprAtom::Number(n) => CellValue::Number(n),
            ExprAtom::Text(s) => CellValue::Text(s.into()),
            ExprAtom::Bool(b) => CellValue::Bool(b),
            ExprAtom::InvalidReferenceError(s) => CellValue::err(s),
            ExprAtom::Function(_) | ExprAtom::Reference(_) => CellValue::err("#VALUE!"),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum AtomType {
    Bool,
    Number,
    Text,
    InvalidReferenceError,
    Function,
    Reference,
}

impl fmt::Display for AtomType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            AtomType::Bool => write!(f, "bool"),
            AtomType::Number => write!(f, "number"),
            AtomType::Text => write!(f, "text"),
            AtomType::InvalidReferenceError => write!(f, "error"),
            AtomType::Function => write!(f, "function"),
            AtomType::Reference => write!(f, "reference"),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum Expr {
    Atom(ExprAtom),
    Negate(ExprId),
    Add(ExprId, ExprId),
    Subtract(ExprId, ExprId),
    Multiply(ExprId, ExprId),
    Divide(ExprId, ExprId),
    Equal(ExprId, ExprId),
    GreaterThan(ExprId, ExprId),
    LessThan(ExprId, ExprId),

    // default functions
    Sum(ExprId),
    Avg(ExprId),
    Min(ExprId),
    Max(ExprId),
    Count(ExprId, ExprId),
    If(ExprId, ExprId, ExprId),

    ExternalFunctionCall {
        func_id: UserFuncId,
        args: Vec<ExprId>,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FormulaTemplateRef {
    pub expr_id: ExprId,
    pub start: u32,
    pub end: u32,
}

/// Repressentation of formula. If multiple cells contain the same FormulaId, then they share single formula
///
/// When new formula is entered by user, it is parsed and preserved as template. During parsing,
/// all references are converted into R1C1 format (meaning AST stores relative offsets, instead
/// of exact IDs of cells).
/// If the formula is cloned, then it becomes shared formula (multiple cells will contain same FormulaId)
#[derive(Serialize, Deserialize, Clone)]
pub struct Formula {
    /// Abstract syntax tree (result of parsing `formula_string_template`)
    pub ast: Vec<Expr>,

    /// Template text, entered by user to create this formula (preserves spaces)
    ///
    /// Constant parts are copied as is. Reference spans are created from AST.
    pub formula_string_template: String,

    /// Byte spans of reference text in `formula_string_template`.
    pub template_refs: Vec<FormulaTemplateRef>,

    /// Byte spans (start, end) for each ExprId in `ast`, relative to `formula_string_template[1..]`.
    /// Used for error highlighting.
    pub spans: Vec<(u32, u32)>,
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

// todo: remove projection, move fields into Table

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Projection {
    pub sheet_id: SheetId,
    pub projection_start: GridCellId,
    pub projection_end: GridCellId,
    pub projected_rows: Vec<u32>,
    pub active: bool,
    #[serde(with = "vec_btreemap_as_vec")]
    pub filter_options_per_column: Vec<BTreeMap<CellValue, ProjectionFilterOption>>,
    pub hidden_rows_count: u32,
    pub filter_show_blanks: Vec<bool>,
    pub next_filter_option_id: u32,
}

// registered JS function that can be called from formulas
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ExternalFunction {
    pub name: String,
    pub args: Vec<AtomType>,
    #[serde(default)]
    pub file_name: String,
}

// extension .js file attached to the spreadsheet
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ScriptFile {
    pub name: String,
    pub content: String,
}

#[derive(Serialize, Deserialize, Default)]
pub struct Sheet {
    pub grid: Grid,
    #[serde(default)]
    pub cell_properties: CellProperties,
}

pub type Sheets = Vec<Sheet>;

#[derive(Serialize, Deserialize)]
pub struct Spreadsheet {
    pub(crate) sheets: Arc<RwLock<Sheets>>,
    pub(crate) formulas: StableVec<Formula>,
    pub(crate) names: SpreadsheetNames,
    pub(crate) tables: StableVec<Table>,
    pub(crate) projections: StableVec<Projection>,
    pub(crate) external_functions: StableVec<ExternalFunction>,
    #[serde(skip, default)]
    pub(crate) scripts: Vec<ScriptFile>,
    #[serde(skip, default)]
    pub(crate) dependency_graph: DependencyGraph,
}

impl Spreadsheet {
    pub fn new() -> Self {
        Self {
            sheets: Arc::new(RwLock::new(vec![Sheet::default()])),
            formulas: StableVec::new(),
            names: SpreadsheetNames::new(),
            tables: StableVec::new(),
            projections: StableVec::new(),
            external_functions: StableVec::new(),
            scripts: Vec::new(),
            dependency_graph: DependencyGraph::new(),
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

    // todo: put all logic for getting viewport in one place
    pub fn get_projected_cell_id(&self, id: &AbsoluteCellId) -> AbsoluteCellId {
        // projected tables display a different stored row than the visible row.
        if let Some((_, table)) = self.find_table_containing_cell(id) {
            // missing projection should not break normal cell display.
            if let Some(projection) = self.projections.get(table.projection_id) {
                // inactive projection means visual row is already the stored row.
                if projection.active {
                    let visual_idx = (id.row - projection.projection_start.row) as usize;
                    // table bounds can be stale during edits, so stay inside projected rows.
                    if visual_idx < projection.projected_rows.len() {
                        return AbsoluteCellId {
                            sheet_id: id.sheet_id,
                            row: projection.projected_rows[visual_idx],
                            col: id.col,
                        };
                    }
                }
            }
        }
        *id
    }

    // todo: remove this mess.

    pub fn get_cell(&self, id: &AbsoluteCellId) -> Option<Cell> {
        self.sheets.read()[id.sheet_id as usize]
            .grid
            .get_cell(&id.into())
    }

    pub fn set_value_and_create_block(&self, id: &AbsoluteCellId, val: CellValue) {
        self.sheets.write()[id.sheet_id as usize]
            .grid
            .set_value_and_create_block(&id.into(), val);
    }

    // shared access: per-cell write lock, no block creation. for parallel eval.
    pub fn set_value(&self, id: &AbsoluteCellId, val: CellValue) {
        self.sheets.read()[id.sheet_id as usize]
            .grid
            .set_value(&id.into(), val);
    }

    pub fn insert_cell(&self, id: &AbsoluteCellId, cell: Cell) {
        self.sheets.write()[id.sheet_id as usize]
            .grid
            .insert_cell(&id.into(), cell);
    }

    pub fn remove_cell(&self, id: &AbsoluteCellId) {
        self.sheets.write()[id.sheet_id as usize]
            .grid
            .remove_cell(&id.into());
    }

    // todo: fix
    pub fn rebuild_dependency_graph(&mut self) {
        let mut formula_cells = Vec::new();
        for (sheet_id, sheet) in self.sheets.read().iter().enumerate() {
            sheet.grid.for_each_cell(|grid_id, cell| {
                if let Some(formula_id) = cell.defined_by_formula {
                    formula_cells.push((
                        AbsoluteCellId {
                            sheet_id: sheet_id as u32,
                            row: grid_id.row,
                            col: grid_id.col,
                        },
                        formula_id,
                    ));
                }
            });
        }

        self.dependency_graph.clear();
        for (cell_id, formula_id) in formula_cells {
            if let Some(formula) = self.formulas.get(formula_id) {
                self.dependency_graph
                    .insert_formula_cell(cell_id, &formula.ast);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(sr: u32, sc: u32, er: u32, ec: u32) -> CellRange {
        CellRange::new(0, sr, sc, er, ec)
    }

    #[test]
    fn is_single() {
        assert!(r(1, 1, 1, 1).is_single());
        assert!(!r(1, 1, 1, 2).is_single());
    }

    #[test]
    fn is_line() {
        assert!(r(1, 1, 1, 5).is_line());
        assert!(r(1, 1, 5, 1).is_line());
        assert!(!r(1, 1, 2, 2).is_line());
        assert!(r(3, 3, 3, 3).is_line()); // single cell is a line
    }

    #[test]
    fn intersection_overlap() {
        let a = r(0, 0, 5, 5);
        let b = r(3, 3, 8, 8);
        assert_eq!(a.intersection(&b), Some(r(3, 3, 5, 5)));
    }

    #[test]
    fn intersection_disjoint() {
        assert_eq!(r(0, 0, 1, 1).intersection(&r(3, 3, 4, 4)), None);
    }

    #[test]
    fn contains() {
        let outer = r(0, 0, 10, 10);
        assert!(outer.contains(&r(2, 2, 5, 5)));
        assert!(!outer.contains(&r(2, 2, 15, 5)));
    }

    #[test]
    fn subtract_full() {
        let a = r(1, 1, 3, 3);
        assert!(a.subtract(&a).is_empty());
    }

    #[test]
    fn subtract_no_overlap() {
        let a = r(0, 0, 2, 2);
        let b = r(5, 5, 6, 6);
        assert_eq!(a.subtract(&b), vec![a]);
    }

    #[test]
    fn subtract_partial() {
        let a = r(0, 0, 3, 3);
        let b = r(0, 0, 1, 3); // top two rows
        let remaining = a.subtract(&b);
        assert!(!remaining.is_empty());
        // remaining should cover rows 2-3
        assert!(remaining.iter().all(|r| r.start_row >= 2));
    }

    #[test]
    fn bounding_union() {
        let a = r(1, 1, 3, 3);
        let b = r(5, 5, 7, 7);
        let u = a.bounding_union(&b);
        assert_eq!(u, r(1, 1, 7, 7));
    }

    #[test]
    fn shifted() {
        let a = r(2, 3, 4, 5);
        assert_eq!(a.shifted(1, -1), Some(r(3, 2, 5, 4)));
        assert_eq!(r(0, 0, 0, 0).shifted(-1, 0), None); // underflow
    }

    #[test]
    fn for_each_cell_count() {
        let a = r(0, 0, 2, 2);
        let mut count = 0;
        a.for_each_cell(|_| count += 1);
        assert_eq!(count, 9); // 3x3
    }

    #[test]
    fn expratom_cellvalue_roundtrip() {
        let cases = [
            CellValue::Number(rust_decimal::Decimal::from(42)),
            CellValue::Text("hi".into()),
            CellValue::Bool(true),
            CellValue::err("oops"),
        ];
        for val in &cases {
            let atom = ExprAtom::from(val.clone());
            let back = CellValue::from(atom);
            match (val, &back) {
                (CellValue::Error(a, _), CellValue::Error(b, _)) => assert_eq!(a, b),
                _ => assert_eq!(val, &back),
            }
        }
    }
}
