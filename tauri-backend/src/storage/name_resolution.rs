use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::{
    parser::string_is_regular_cell_name,
    storage::{
        grid::{Cell, GridCellId},
        types::{
            AbsoluteCellId, CellRange, Coordinate, Reference, SheetId, Sheets, TableId, UserFuncId,
        },
    },
};

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

    #[serde(default)]
    pub table_columns: HashMap<(TableId, String), Reference>,
    #[serde(default)]
    pub table_columns_lookup: HashMap<CellRange, (TableId, String)>,

    #[serde(default)]
    pub user_function_names: HashMap<String, UserFuncId>,
    #[serde(default)]
    pub user_function_names_lookup: HashMap<UserFuncId, String>,
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
            table_columns: HashMap::new(),
            table_columns_lookup: HashMap::new(),
            user_function_names: HashMap::new(),
            user_function_names_lookup: HashMap::new(),
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

    /// Assign a custom name to a cell.
    pub fn create_cell_name(&mut self, name: &str, id: &AbsoluteCellId) -> Option<()> {
        // empty name: remove existing custom name
        if name.is_empty() {
            if let Some(old_name) = self.cell_names_lookup.remove(id) {
                self.cell_names.remove(&old_name);
            }
            return Some(());
        }

        // reject names that look like regular cell references
        if string_is_regular_cell_name(name) {
            return None;
        }

        // reject if another cell already has this name
        if let Some(_) = self.cell_names.get(name) {
            return None;
        }

        // remove old name for this cell if it had one
        if let Some(old_name) = self.cell_names_lookup.remove(id) {
            self.cell_names.remove(&old_name);
        }

        self.cell_names.insert(name.to_string(), *id);
        self.cell_names_lookup.insert(*id, name.to_string());
        Some(())
    }

    // move existing cell name from source cell to dest cell
    pub fn move_cell_name(&mut self, source: &AbsoluteCellId, dest: &AbsoluteCellId) {
        if source == dest {
            return;
        }

        let Some(name) = self.cell_names_lookup.remove(source) else {
            return;
        };

        if let Some(old_name) = self.cell_names_lookup.remove(dest) {
            self.cell_names.remove(&old_name);
        }

        self.cell_names.insert(name.clone(), *dest);
        self.cell_names_lookup.insert(*dest, name);
    }

    pub fn create_table_name_and_columns(
        &mut self,
        sheets: &mut Sheets,
        table_id: TableId,
        table_name: &str,
        sheet_id: SheetId,
        first_header: GridCellId,
        body_start: GridCellId,
        body_end: GridCellId,
    ) {
        // table names are part of formula name resolution
        self.table_names.insert(table_name.to_string(), table_id);
        self.table_names_lookup
            .insert(table_id, table_name.to_string());

        // register every table column right after the table exists
        for col_idx in 0..=(body_end.col - body_start.col) {
            let header = GridCellId {
                row: first_header.row,
                col: first_header.col + col_idx,
            };
            self.sync_table_column(
                sheets,
                table_id,
                sheet_id,
                header,
                body_start,
                body_end,
                col_idx as usize,
            );
        }
    }

    pub fn sync_table_column(
        &mut self,
        sheets: &mut Sheets,
        table_id: TableId,
        sheet_id: SheetId,
        header: GridCellId,
        body_start: GridCellId,
        body_end: GridCellId,
        col_idx: usize,
    ) {
        // column name is header text, but with spaces replaced with underscore _
        let header_value = sheets[sheet_id as usize].get_value(&header);
        let raw_name = header_value.map(|v| v.to_string()).unwrap_or_default();
        let empty_header = raw_name.is_empty();
        // spaces are written as underscores in formulas
        let column_name = if empty_header {
            format!("Column{}", col_idx + 1)
        } else {
            raw_name.replace(' ', "_")
        };

        // if any header is empty, replace it with default column name
        if empty_header {
            sheets[sheet_id as usize].insert_cell(&header, Cell::text(column_name.clone()));
        }

        // table columns resolve to body ranges
        let range = CellRange::new(
            sheet_id,
            body_start.row,
            header.col,
            body_end.row,
            header.col,
        );
        let reference = Reference::Range {
            sheet_id,
            start_row: Coordinate::Absolute(body_start.row),
            start_col: Coordinate::Absolute(header.col),
            end_row: Coordinate::Absolute(body_end.row),
            end_col: Coordinate::Absolute(header.col),
        };
        // if column name did not change, return
        if self
            .table_columns_lookup
            .get(&range)
            .is_some_and(|(old_table_id, old_name)| {
                *old_table_id == table_id && old_name == &column_name
            })
        {
            return;
        }

        // otherwise, update mappings
        if let Some((old_table_id, old_name)) = self.table_columns_lookup.remove(&range) {
            self.table_columns.remove(&(old_table_id, old_name));
        }
        self.table_columns
            .insert((table_id, column_name.clone()), reference.clone());
        self.table_columns_lookup
            .insert(range, (table_id, column_name.clone()));
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
