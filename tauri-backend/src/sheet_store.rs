// todo: figure out efficent dependency storage (rstar?)
// maybe also help calculating active counts easier?
// figure out cell storage (move away from btreemap)
// figure out shared formulas
// figure out cell addresing and resolution (store offsets in references insetad of CellId?)
// figure out string storage (reduce allocations)

use std::collections::HashMap;

use crate::sheet::{Cell, SheetId, UserFuncId, UserFunction};

pub type Block = [[Option<Cell>; 32]; 32];

pub type Sheet = Vec<Block>;

pub struct CellId {
    sheet_id: u32,
    block_id: u32,
    col: u32,
    row: u32,
}

pub struct Store {
    pub sheets: Vec<Sheet>,

    pub sheet_names: HashMap<String, SheetId>,
    pub sheet_names_lookup: HashMap<SheetId, String>,

    pub cell_names: HashMap<String, CellId>,
    pub cell_names_lookup: HashMap<CellId, String>,

    pub user_functions: Vec<UserFunction>,
    pub user_function_names: HashMap<String, UserFuncId>,
    pub user_function_names_lookup: HashMap<UserFuncId, String>,
}
