use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};

use crate::sheet::{CellId, Formula, NameRef, OptSheetId, Spreadsheet, UserFunction};

const SHEET_NAMES_PATH: &str = "nametable/sheet_names.csv";
const CELL_NAMES_PATH: &str = "nametable/cell_names.csv";
const USER_FUNCTION_NAMES_PATH: &str = "nametable/function_names.csv";
const FORMULAS_RAW_TEXT_PATH: &str = "meta/formulas_raw_text.csv";
const USER_FUNCTIONS_PATH: &str = "meta/user_functions.csv";

// todo: move to parser.rs
fn parse_name_ref(s: &str) -> NameRef {
    if let Some((id_str, name)) = s.split_once('.') {
        if let Ok(id) = id_str.parse::<u32>() {
            return NameRef {
                sheet_id: OptSheetId::Id(id),
                name: name.to_string(),
            };
        }
    }
    NameRef {
        sheet_id: OptSheetId::None,
        name: s.to_string(),
    }
}

/// Save spreadhseet into `path` (file extension should be in the path)
/// todo: saving sheet with all IDs (for names, functions, etc) instead of
///       just strings feels messy and not sure if worth it
pub fn save(spreadsheet: &Spreadsheet, path: &str) -> std::io::Result<()> {
    // create spreadsheet directory
    std::fs::create_dir(path)?;

    // save each logical sheet representation (BTreeMap) to [sheet_name].csv
    // uses IDs instead of Names to store reference to something (like other cell, function, etc)
    for (sheet_id, sheet) in spreadsheet.sheets.iter().enumerate() {
        let sheet_name = &spreadsheet.sheet_names_lookup[&(sheet_id as u32)]; // &(sheet_id as u32)  <-- ???
        let sheet_path = format!("{}/{}.csv", path, sheet_name);
        let mut file = File::create(&sheet_path)?;
        for (cell_id, formula) in sheet {
            let col = cell_id.col;
            let row = cell_id.row;
            writeln!(file, "{},{},{}", col, row, formula);
        }
    }

    // save name table of cells
    let mut f = File::create(format!("{}/{}", path, CELL_NAMES_PATH))?;
    for (name, cell_id) in &spreadsheet.cells_names {
        writeln!(f, "{},{},{}", cell_id.col, cell_id.row, name)?;
    }

    // save name table of sheets
    let mut f = File::create(format!("{}/{}", path, SHEET_NAMES_PATH))?;
    for (name, id) in &spreadsheet.sheet_names {
        writeln!(f, "{},{}", id, name)?;
    }

    // save name table of user functions
    let mut f = File::create(format!("{}/{}", path, USER_FUNCTION_NAMES_PATH))?;
    for (name, id) in &spreadsheet.user_functions_names {
        writeln!(f, "{},{}", id, name)?;
    }

    // save user functions
    let mut f = File::create(format!("{}/{}", path, USER_FUNCTIONS_PATH))?;
    for (id, func) in spreadsheet.user_functions.iter().enumerate() {
        writeln!(f, "{},{}", id, func)?;
    }

    // save raw text of formulas (what user typed)
    let mut f = File::create(format!("{}/{}", path, FORMULAS_RAW_TEXT_PATH))?;
    for (cell_id, name) in &spreadsheet.formulas_raw_text {
        // cell_id is displayed as (cell_id.col, cell_id.row)
        // todo: too ambigous?
        writeln!(f, "{},{}", cell_id, name)?;
    }

    Ok(())
}

/// Loads a spreadsheet from the directory at `path`.
///
/// * `parse_formula` - Parses a raw cell string into a [`Formula`]
/// * `parse_user_function` - Parses a textual representation of a user function string into a [`UserFunction`]
/// todo: remove code duplication
pub fn load(
    path: &str,
    parse_formula: impl Fn(&str) -> Formula,
    parse_user_function: impl Fn(&str) -> UserFunction,
) -> io::Result<Spreadsheet> {
    // allocate spreadsheet fields
    let mut sheets = Vec::new();
    let mut sheet_names = HashMap::new();
    let mut sheet_names_lookup = HashMap::new();
    let mut cells_names = HashMap::new();
    let mut cells_names_lookup = HashMap::new();
    let mut user_functions = Vec::new();
    let mut user_functions_names = HashMap::new();
    let mut user_functions_names_lookup = HashMap::new();
    let mut formulas_raw_text = HashMap::new();

    let mut buf = String::new(); // re-used buffer

    // load sheet_names and sheet_names_lookup
    let name = format!("{}/{}", path, SHEET_NAMES_PATH);
    let mut reader = BufReader::new(File::open(&name)?);
    while reader.read_line(&mut buf)? > 0 {
        let mut iter = buf.trim().split(',');
        if let (Some(s1), Some(s2)) = (iter.next(), iter.next()) {
            let sheet_id: u32 = s1
                .trim()
                .parse()
                .expect("first column of sheet_names.csv to be integer (u32)");
            let sheet_name: String = s2.trim().to_string();
            sheet_names.insert(sheet_name.clone(), sheet_id);
            sheet_names_lookup.insert(sheet_id, sheet_name);
        }
        buf.clear();
    }

    // load cells_names and cells_names_lookup
    let name = format!("{}/{}", path, CELL_NAMES_PATH);
    let mut reader = BufReader::new(File::open(&name)?);
    while reader.read_line(&mut buf)? > 0 {
        let mut iter = buf.trim().split(',');
        if let (Some(s1), Some(s2), Some(s3)) = (iter.next(), iter.next(), iter.next()) {
            let col: u32 = s1
                .trim()
                .parse()
                .expect("first column of cell_names.csv to be integer (u32)");
            let row: u32 = s2
                .trim()
                .parse()
                .expect("second column of cell_names.csv to be integer (u32)");
            let name_str: String = s3.trim().to_string();
            let cell_id = CellId { col, row };
            cells_names.insert(parse_name_ref(&name_str), cell_id);
            cells_names_lookup.insert(cell_id, parse_name_ref(&name_str));
        }
        buf.clear();
    }

    // load user_functions_names and user_functions_names_lookup
    let name = format!("{}/{}", path, USER_FUNCTION_NAMES_PATH);
    let mut reader = BufReader::new(File::open(&name)?);
    while reader.read_line(&mut buf)? > 0 {
        let mut iter = buf.trim().split(',');
        if let (Some(s1), Some(s2)) = (iter.next(), iter.next()) {
            let func_id: u32 = s1
                .trim()
                .parse()
                .expect("first column of function_names.csv to be integer (u32)");
            let func_name = s2.trim().to_string();
            user_functions_names.insert(parse_name_ref(&func_name), func_id);
            user_functions_names_lookup.insert(func_id, parse_name_ref(&func_name));
        }
        buf.clear();
    }

    // load user_functions
    let name = format!("{}/{}", path, USER_FUNCTIONS_PATH);
    let mut reader = BufReader::new(File::open(&name)?);
    while reader.read_line(&mut buf)? > 0 {
        let mut iter = buf.trim().split(',');
        if let (Some(s1), Some(s2)) = (iter.next(), iter.next()) {
            let id: u32 = s1
                .trim()
                .parse()
                .expect("first column of user_functions.csv to be integer (u32)");
            let func_text = s2.trim().to_string();
            user_functions.insert(id as usize, parse_user_function(&func_text));
        }
        buf.clear();
    }

    // load text of formulas (raw user input)
    let name = format!("{}/{}", path, FORMULAS_RAW_TEXT_PATH);
    let mut reader = BufReader::new(File::open(&name)?);
    while reader.read_line(&mut buf)? > 0 {
        let mut iter = buf.trim().split(',');
        if let (Some(s1), Some(s2), Some(s3)) = (iter.next(), iter.next(), iter.next()) {
            let col: u32 = s1
                .trim()
                .parse()
                .expect("first column of formulas_raw_text.csv to be integer (u32)");
            let row: u32 = s2
                .trim()
                .parse()
                .expect("second column of formulas_raw_text.csv to be integer (u32)");
            let formula_raw_text: String = s3.trim().to_string();
            formulas_raw_text.insert(CellId { col, row }, formula_raw_text);
        }
        buf.clear();
    }

    // load sheets
    for (sheet_name, _id) in &sheet_names {
        let name = format!("{}/{}.csv", path, sheet_name);
        let reader = BufReader::new(File::open(&name)?);
        let mut sheet = BTreeMap::new();
        for line in reader.lines() {
            let line = line?;
            let mut parts = line.splitn(3, ',');
            let col: u32 = parts.next().expect("col").parse().expect("col u32");
            let row: u32 = parts.next().expect("row").parse().expect("row u32");
            let formula_text = parts.next().expect("value data");
            let cell_id = CellId { col, row };
            let formula = parse_formula(formula_text);
            sheet.insert(cell_id, formula);
        }
        sheets.push(sheet);
    }

    Ok(Spreadsheet {
        sheets,
        sheet_names,
        sheet_names_lookup,
        cells_names,
        cells_names_lookup,
        user_functions,
        user_functions_names,
        user_functions_names_lookup,
        formulas_raw_text,
    })
}
