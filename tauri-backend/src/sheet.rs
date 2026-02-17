use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

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

#[derive(Serialize, Deserialize, Eq, PartialEq, Hash, Clone, Debug)]
pub enum OptSheetId {
    Id(SheetId),
    None,
}

pub type UserFuncId = u32;
pub type ExprId = u32;

#[derive(Serialize, Deserialize, Eq, PartialEq, Hash, Ord, PartialOrd, Clone, Copy, Debug)]
pub struct CellId {
    pub col: u32,
    pub row: u32,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct CellRange {
    pub start: CellId,
    pub end: CellId,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum ExprValue {
    Number(f64),
    Text(String),
    Function(UserFuncId),
    CellRef(OptSheetId, CellId),
    RelativeCellRef(OptSheetId, CellId),
    CellRange(OptSheetId, CellRange),
    RelativeCellRange(OptSheetId, CellRange),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum ExprType {
    Number,
    Text,
    Function,
    CellRef,
    RelativeCellRef,
    CellRange,
    RelativeCellRange,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum Expr {
    Literal(ExprValue),
    Add(ExprId, ExprId),
    Subtract(ExprId, ExprId),
    Multiply(ExprId, ExprId),
    Divide(ExprId, ExprId),
    FunctionCall(Func, Vec<Expr>),
}

// formula is either single value or expression
// if single value: size is fixed (unless string or smth like that),
// if expression: size is dynamic
#[derive(Serialize, Deserialize, Debug)]
pub enum Formula {
    SingleValue(ExprValue),
    Expression(Vec<Expr>),
}

#[derive(Serialize, Deserialize, Debug)]
pub enum Func {
    Sum,
    Avg,
    ExprFunction(UserFuncId),
    // todo JsFunction(UserFuncId),
}

#[derive(Serialize, Deserialize, Debug)]
pub struct UserFunction {
    pub args_names: Vec<String>,
    pub args_types: Vec<ExprType>,
    pub return_type: ExprType,
    pub exprs: Vec<Expr>,
    // todo pub js_callback
}

impl Default for UserFunction {
    fn default() -> Self {
        Self {
            args_names: Vec::new(),
            args_types: Vec::new(),
            return_type: ExprType::Number,
            exprs: Vec::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Eq, PartialEq, Hash, Clone, Debug)]
pub struct NameRef {
    pub sheet_id: OptSheetId,
    pub name: String,
}

// todo: NameRef != CellId, because CellId does not have sheet index

//
// todo: engine uses IDs (CellId, SheetId, UserFuncId) when it needs to find cell,
// sheet index (maybe engine doesn't need sheet indx?), and user functions instead of real text names.
// so when we try to parse file or user input, we need to know their ID to pass it to engine.
// todo:Maybe save egnine strucure and name tables separately
// todo: covert hashmaps with u32 as key to regular Vec
//

#[derive(Serialize, Deserialize, Debug)]
pub struct Spreadsheet {
    pub sheets: Vec<BTreeMap<CellId, Formula>>,

    pub sheet_names: HashMap<String, SheetId>,
    pub sheet_names_lookup: HashMap<SheetId, String>,

    pub cells_names: HashMap<NameRef, CellId>,
    pub cells_names_lookup: HashMap<CellId, NameRef>,

    pub user_functions: Vec<UserFunction>,
    pub user_functions_names: HashMap<NameRef, UserFuncId>,
    pub user_functions_names_lookup: HashMap<UserFuncId, NameRef>,

    pub formulas_raw_text: HashMap<CellId, String>,
}

// pub fn number_to_letter(n: u32) -> char {
//     std::char::from_u32(65 + n).expect("n to be convertable to letter")
// }

// pub fn letter_to_number(c: char) -> u32 {
//     (c as u32) - 65
// }
