use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::storage::types::{AbsoluteCellId, SheetId, TableId};

#[derive(Serialize, Deserialize)]
pub struct SpreadsheetNames {
    pub sheet_names: HashMap<String, SheetId>,
    pub sheet_names_lookup: HashMap<SheetId, String>,

    pub cell_names: HashMap<String, AbsoluteCellId>,
    pub cell_names_lookup: HashMap<AbsoluteCellId, String>,

    #[serde(default)]
    pub table_names: HashMap<String, TableId>,
    #[serde(default)]
    pub table_names_lookup: HashMap<TableId, String>,
    // todo:
    // pub user_function_names: HashMap<String, UserFuncId>,
    // pub user_function_names_lookup: HashMap<UserFuncId, String>,
}

impl SpreadsheetNames {
    pub fn new() -> Self {
        Self {
            sheet_names: HashMap::new(),
            sheet_names_lookup: HashMap::new(),
            cell_names: HashMap::new(),
            cell_names_lookup: HashMap::new(),
            table_names: HashMap::new(),
            table_names_lookup: HashMap::new(),
        }
    }

    /// Convert AbsoluteCellId to A1-style name (e.g., "A1", "AB200").
    /// If the cell has a named reference, return that name instead.
    pub fn cell_id_to_name(&self, id: &AbsoluteCellId) -> String {
        if let Some(name) = self.cell_names_lookup.get(id) {
            return name.clone();
        }
        col_to_letters(id.col) + &(id.row + 1).to_string()
    }
}

/// Convert 0-indexed column to letter(s): 0->A, 25->Z, 26->AA
fn col_to_letters(col: u32) -> String {
    let mut result = String::new();
    let mut c = col;
    loop {
        result.insert(0, (b'A' + (c % 26) as u8) as char);
        if c < 26 {
            break;
        }
        c = c / 26 - 1;
    }
    result
}
