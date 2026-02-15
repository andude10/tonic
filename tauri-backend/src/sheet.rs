use core::fmt;
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

#[derive(Eq, PartialEq, Hash)]
pub enum OptSheetId {
    Id(SheetId),
    None,
}

pub type UserFuncId = u32;

impl fmt::Display for NameRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.sheet_id {
            OptSheetId::Id(id) => write!(f, "{}.{}", id, self.name),
            OptSheetId::None => write!(f, "{}", self.name),
        }
    }
}

impl fmt::Display for CellId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{},{}", self.col, self.row)
    }
}

// todo: use Display for display user-facing strings (like A5 instead of 0,5)?

impl fmt::Display for Formula {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Formula::SingleValue(expr_value) => write!(f, "{}", expr_value),
            Formula::Expression(exprs) => {
                for expr in exprs {
                    write!(f, "{}", expr)?;
                }
                Ok(())
            }
        }
    }
}

impl fmt::Display for ExprValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExprValue::Number(num) => write!(f, "num/{}", num),
            ExprValue::Text(text) => write!(f, "text/{}", text),
            ExprValue::Function(func_id) => {
                write!(f, "{}", func_id)
            }
            ExprValue::CellRef(opt_sheet_id, cell_id) => match opt_sheet_id {
                OptSheetId::Id(id) => write!(f, "{}.{}", id, cell_id),
                OptSheetId::None => write!(f, "{}", cell_id),
            },
            ExprValue::RelativeCellRef(opt_sheet_id, cell_id) => match opt_sheet_id {
                OptSheetId::Id(id) => write!(f, "{}.~{}", id, cell_id),
                OptSheetId::None => write!(f, "~{}", cell_id),
            },
            ExprValue::CellRange(opt_sheet_id, cell_range) => match opt_sheet_id {
                OptSheetId::Id(id) => write!(f, "{}.{}:{}", id, cell_range.start, cell_range.end),
                OptSheetId::None => write!(f, "{}:{}", cell_range.start, cell_range.end),
            },
            ExprValue::RelativeCellRange(opt_sheet_id, cell_range) => match opt_sheet_id {
                OptSheetId::Id(id) => write!(f, "{}.~{}:{}", id, cell_range.start, cell_range.end),
                OptSheetId::None => write!(f, "~{}:{}", cell_range.start, cell_range.end),
            },
        }
    }
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Expr::Literal(a) => write!(f, "{}", a),
            Expr::Add(a, b) => write!(f, "{}+{}", a, b),
            Expr::Subtract(a, b) => write!(f, "{}-{}", a, b),
            Expr::Multiply(a, b) => write!(f, "{}*{}", a, b),
            Expr::Divide(a, b) => write!(f, "{}/{}", a, b),
            Expr::FunctionCall(func, exprs) => {
                write!(f, "{}(", func)?;
                for (i, expr) in exprs.iter().enumerate() {
                    write!(f, "{}", expr)?;
                    if i < exprs.len() - 1 {
                        write!(f, ", ")?;
                    }
                }
                write!(f, ")")
            }
        }
    }
}

impl fmt::Display for Func {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Func::Sum => write!(f, "sum"),
            Func::Avg => write!(f, "avg"),
            Func::ExprFunction(user_func_id) => write!(f, "func/{}", user_func_id),
        }
    }
}

impl fmt::Display for ExprType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExprType::Number => write!(f, "num"),
            ExprType::Text => write!(f, "text"),
            ExprType::Function => write!(f, "func"),
            ExprType::CellRef => write!(f, "cell"),
            ExprType::RelativeCellRef => write!(f, "rel_cell"),
            ExprType::CellRange => write!(f, "range"),
            ExprType::RelativeCellRange => write!(f, "rel_range"),
        }
    }
}

impl fmt::Display for UserFunction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "arg_names/")?;
        for (i, name) in self.args_names.iter().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }
            write!(f, "{}", name)?;
        }
        write!(f, "/arg_types/")?;
        for (i, et) in self.args_types.iter().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }
            write!(f, "{}", et)?;
        }
        write!(f, "/return_type/{}/exprs/", self.return_type)?;
        for (i, e) in self.exprs.iter().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }
            write!(f, "{}", e)?;
        }
        Ok(())
    }
}

pub type ExprId = u32;

#[derive(Eq, PartialEq, Hash, Ord, PartialOrd, Clone, Copy)]
pub struct CellId {
    pub col: u32,
    pub row: u32,
}

pub struct CellRange {
    pub start: CellId,
    pub end: CellId,
}

pub enum ExprValue {
    Number(f64),
    Text(String),
    Function(UserFuncId),
    CellRef(OptSheetId, CellId),
    RelativeCellRef(OptSheetId, CellId),
    CellRange(OptSheetId, CellRange),
    RelativeCellRange(OptSheetId, CellRange),
}

pub enum ExprType {
    Number,
    Text,
    Function,
    CellRef,
    RelativeCellRef,
    CellRange,
    RelativeCellRange,
}

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
pub enum Formula {
    SingleValue(ExprValue),
    Expression(Vec<Expr>),
}

enum Func {
    Sum,
    Avg,
    ExprFunction(UserFuncId),
    // todo JsFunction(UserFuncId),
}

pub struct UserFunction {
    pub args_names: Vec<String>,
    pub args_types: Vec<ExprType>,
    pub return_type: ExprType,
    pub exprs: Vec<Expr>,
    // todo pub js_callback
}

impl UserFunction {
    pub fn new() -> Self {
        Self {
            args_names: Vec::new(),
            args_types: Vec::new(),
            return_type: ExprType::Number,
            exprs: Vec::new(),
        }
    }
}

#[derive(Eq, PartialEq, Hash)]
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
