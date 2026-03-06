use std::{
    collections::{BTreeMap, HashMap},
    fmt,
};

use fastnum::D256;
use serde::{Deserialize, Serialize};

// #[macro_export]
// macro_rules! display_cell {
//     ($cell:expr) => {{
//         let col_char =
//             std::char::from_u32(65 + $cell.col).expect("Column index out of bounds (0-25)");
//         format!("{}{}", col_char, $cell.row + 1)
//     }};
// }
// #[macro_export]
// macro_rules! display_expr_value {
//     ($spreadsheet:expr, $expr:expr) => {{
//         match $expr {
//             ExprValue::Number(num) => format!("{}", num),
//             ExprValue::Text(text) => text.clone(),
//             ExprValue::Function(func_id) => {
//                 format!("{}", $spreadsheet.user_functions_names_lookup[func_id])
//             }
//             ExprValue::CellRef(opt_sheet_id, cell_id) => match opt_sheet_id {
//                 $crate::sheet::OptSheetId::Id(id) => format!(
//                     "{}.{}",
//                     $spreadsheet.sheets_names_reverse[*id as usize],
//                     $crate::format_cell!(cell_id)
//                 ),
//                 $crate::sheet::OptSheetId::None => $crate::format_cell!(cell_id),
//             },
//             ExprValue::NamedCellRef(name_ref) => match name_ref.sheet_id {
//                 $crate::sheet::OptSheetId::Id(id) => format!(
//                     "{}.{}",
//                     $spreadsheet.sheets_name_table[id as usize], name_ref.name
//                 ),
//                 $crate::sheet::OptSheetId::None => format!("{}", name_ref.name),
//             },
//             ExprValue::RelativeCellRef(opt_sheet_id, cell_id) => match opt_sheet_id {
//                 $crate::sheet::OptSheetId::Id(id) => format!(
//                     "{}.~{}",
//                     $spreadsheet.sheets_name_table[*id as usize],
//                     $crate::format_cell!(cell_id)
//                 ),
//                 $crate::sheet::OptSheetId::None => format!("~{}", $crate::format_cell!(cell_id)),
//             },
//             ExprValue::CellRange(opt_sheet_id, cell_range) => match opt_sheet_id {
//                 $crate::sheet::OptSheetId::Id(id) => format!(
//                     "{}.{}:{}",
//                     $spreadsheet.sheets_name_table[*id as usize],
//                     $crate::format_cell!(cell_range.start),
//                     $crate::format_cell!(cell_range.end)
//                 ),
//                 $crate::sheet::OptSheetId::None => format!(
//                     "{}:{}",
//                     $crate::format_cell!(cell_range.start),
//                     $crate::format_cell!(cell_range.end)
//                 ),
//             },
//             ExprValue::RelativeCellRange(opt_sheet_id, cell_range) => match opt_sheet_id {
//                 $crate::sheet::OptSheetId::Id(id) => format!(
//                     "{}.~{}:{}",
//                     $spreadsheet.sheets_name_table[*id as usize],
//                     $crate::format_cell!(cell_range.start),
//                     $crate::format_cell!(cell_range.end)
//                 ),
//                 $crate::sheet::OptSheetId::None => format!(
//                     "~{}:{}",
//                     $crate::format_cell!(cell_range.start),
//                     $crate::format_cell!(cell_range.end)
//                 ),
//             },
//         }
//     }};
// }

pub type SheetId = u32;

pub type UserFuncId = u32;
pub type ExprId = u32;

#[derive(Serialize, Deserialize, Eq, PartialEq, Hash, Ord, PartialOrd, Clone, Copy, Debug)]
pub struct CellId {
    pub col: u32,
    pub row: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub struct CellRange {
    pub start: CellId,
    pub end: CellId,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum ExprAtom {
    Boolean(bool),
    Number(D256),
    Text(String),
    Function(UserFuncId),
    CellRef(SheetId, CellId),
    CellRange(SheetId, CellRange),
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
    Sum {
        range_id: ExprId,
        sum: D256,
    },
    Avg {
        range_id: ExprId,
        count: u64,
        sum: D256,
    },

    ExtrnalFunctionCall {
        func_id: UserFuncId,
        args: Vec<ExprId>,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum CellValue {
    Number(D256),
    Text(String),
}

impl fmt::Display for CellValue {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            CellValue::Number(num) => write!(f, "{}", num),
            CellValue::Text(text) => write!(f, "{}", text),
        }
    }
}

impl fmt::Display for CellId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "(column: {}, row: {})", self.col, self.row)
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub enum Cell {
    SingleValue(CellValue),
    Formula { expr: Vec<Expr>, value: CellValue },
    FormulaError { error: String },
}

#[derive(Serialize, Deserialize, Debug)]
pub struct UserFunction {
    pub args_names: Vec<String>,
    pub args_types: Vec<AtomType>,
    pub return_type: AtomType,
    pub exprs: Vec<Expr>,
    // todo pub js_callback
}

impl Default for UserFunction {
    fn default() -> Self {
        Self {
            args_names: Vec::new(),
            args_types: Vec::new(),
            return_type: AtomType::Number,
            exprs: Vec::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Eq, PartialEq, Hash, Clone, Debug)]
pub struct NameRef {
    pub sheet_id: SheetId,
    pub name: String,
}

impl fmt::Display for NameRef {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.sheet_id == 0 {
            write!(f, "{}", self.name)
        } else {
            write!(f, "{}.{}", self.sheet_id, self.name)
        }
    }
}

//
// todo: engine uses IDs (CellId, SheetId, UserFuncId) when it needs to find cell,
// sheet index (maybe engine doesn't need sheet indx?), and user functions instead of real text names.
// so when we try to parse file or user input, we need to know their ID to pass it to engine.
// todo:Maybe save egnine strucure and name tables separately
// todo: covert hashmaps with u32 as key to regular Vec
//

#[derive(Serialize, Deserialize, Debug)]
pub struct Spreadsheet {
    pub sheets: Vec<BTreeMap<CellId, Cell>>,

    pub sheet_names: HashMap<String, SheetId>,
    pub sheet_names_lookup: HashMap<SheetId, String>,

    pub cell_names: HashMap<NameRef, CellId>,
    pub cell_names_lookup: HashMap<CellId, NameRef>,

    pub user_functions: Vec<UserFunction>,
    pub user_function_names: HashMap<NameRef, UserFuncId>,
    pub user_function_names_lookup: HashMap<UserFuncId, NameRef>,

    pub user_input_raw_text: HashMap<CellId, String>,
}

impl Spreadsheet {
    pub fn get_cell_value(&self, cell_id: &CellId, sheet_id: SheetId) -> Option<&CellValue> {
        match self.sheets.get(sheet_id as usize)?.get(cell_id)? {
            Cell::SingleValue(v) => Some(v),
            Cell::Formula { value, .. } => Some(value),
            Cell::FormulaError { .. } => None,
        }
    }

    pub fn new() -> Self {
        Self {
            sheets: vec![BTreeMap::new()],
            sheet_names: HashMap::new(),
            sheet_names_lookup: HashMap::new(),
            cell_names: HashMap::new(),
            cell_names_lookup: HashMap::new(),
            user_functions: Vec::new(),
            user_function_names: HashMap::new(),
            user_function_names_lookup: HashMap::new(),
            user_input_raw_text: HashMap::new(),
        }
    }
}

// pub fn number_to_letter(n: u32) -> char {
//     std::char::from_u32(65 + n).expect("n to be convertable to letter")
// }

// pub fn letter_to_number(c: char) -> u32 {
//     (c as u32) - 65
// }
