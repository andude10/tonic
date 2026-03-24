// How backend works
//
// Backend exposes tarui commands to the frontend. Every command is not async.
// Frontend polls for cells that are in the current viewport (cells currently
// visible on screen) each 20ms or so (via get_cells_in_viewport command).
//
// When user modifies the spreadsheet, the main thread is blocked until the
// backend reacts to modification (inserts values, recomputes dependencies, etc)

use std::{error::Error, sync::Mutex};

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_log::log::{debug, error};

use std::collections::BTreeMap;

use crate::engine::{ChangeBounds, Engine};
use crate::parser::shift_formula_refs;
use crate::storage::grid::{CellContent, CellValue, GridCellId};
use crate::storage::types::{
    AbsoluteCellId, Projection, ProjectionFilterOption, ProjectionSortOption, Table,
};

mod engine;
mod file_api;
mod parser;
pub(crate) mod storage {
    pub(crate) mod grid;
    pub(crate) mod name_resolution;
    pub(crate) mod stable_vec;
    pub(crate) mod types;
}

/// Frontend cell ID (0-indexed row/col, no sheet). Converted to AbsoluteCellId with sheet_id=0.
#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
struct CellId {
    row: u32,
    col: u32,
}

impl CellId {
    fn to_absolute(self) -> AbsoluteCellId {
        AbsoluteCellId {
            sheet_id: 0,
            row: self.row,
            col: self.col,
        }
    }
}

struct TonicState {
    engine: Engine,
    current_buf: Vec<u8>,
    last_viewport_buf: Vec<u8>,
    last_viewport_range: (u32, u32, u32, u32),
    file_name: Option<String>,
    file_path: Option<String>,
}

impl TonicState {
    fn new() -> Self {
        Self {
            engine: Engine::new(),
            current_buf: Vec::new(),
            last_viewport_buf: Vec::new(),
            last_viewport_range: (u32::MAX, u32::MAX, u32::MAX, u32::MAX),
            file_name: None,
            file_path: None,
        }
    }
}

fn emit_save_status(app: &AppHandle, state: &TonicState) {
    let _ = app.emit("save-status", state.engine.is_saved());
}

fn emit_table_projection_events(
    app: &AppHandle,
    table_id: u32,
    was_active: bool,
    is_active: bool,
    hidden_rows: Option<u32>,
) {
    if !was_active && is_active {
        let _ = app.emit("enable-table-projection", table_id);
    } else if was_active && !is_active {
        let _ = app.emit("disable-table-projection", table_id);
    }
    if let Some(hidden) = hidden_rows {
        let _ = app.emit("update-table-hidden-rows", (table_id, hidden));
    }
}

/// Encode a single cell into the binary buffer.
/// Format: [row: u32 LE][col: u32 LE][is_formula: u8][display_len: u32 LE][display_bytes]
fn encode_cell(buf: &mut Vec<u8>, row: u32, col: u32, content: Option<&CellContent>) {
    buf.extend_from_slice(&row.to_le_bytes());
    buf.extend_from_slice(&col.to_le_bytes());
    match content {
        Some(content) => {
            buf.push(content.defined_by_formula.is_some() as u8);
            let display = content.val.to_string();
            let display_bytes = display.as_bytes();
            buf.extend_from_slice(&(display_bytes.len() as u32).to_le_bytes());
            buf.extend_from_slice(display_bytes);
        }
        None => {
            buf.push(0); // not a formula
            buf.extend_from_slice(&0u32.to_le_bytes()); // empty display
        }
    }
}

// todo: add validation for invalid cell sizes

/// Get the editor value for a cell.
/// For regular data, returns the string representation.
/// For formulas, returns the formula string with shifted references.
fn get_editor_value(state: &TonicState, cell_id: CellId) -> String {
    let abs_id = cell_id.to_absolute();
    let Some(content) = state.engine.spreadsheet.get_content(&abs_id) else {
        return String::new();
    };

    // If defined by formula, shift references (formula_string already includes '=')
    if let Some(formula_id) = content.defined_by_formula {
        if let Some(formula) = state.engine.spreadsheet.formulas.get(formula_id) {
            let gid = GridCellId {
                row: cell_id.row,
                col: cell_id.col,
            };
            return shift_formula_refs(
                &formula.formula_string,
                &gid,
                &formula.ast,
                &state.engine.spreadsheet.names,
            );
        }
    }

    // Otherwise, return the value as string
    content.val.to_string()
}

#[tauri::command(async)]
fn get_editor_value_for_cell(
    state: tauri::State<'_, Mutex<TonicState>>,
    cell_id: CellId,
) -> tauri::ipc::Response {
    let state = state.lock().unwrap();
    let value = get_editor_value(&state, cell_id);
    tauri::ipc::Response::new(value.into_bytes())
}

#[tauri::command(async)]
fn get_name_for_cell(
    state: tauri::State<'_, Mutex<TonicState>>,
    cell_id: CellId,
) -> tauri::ipc::Response {
    let state = state.lock().unwrap();
    let abs_id = cell_id.to_absolute();
    let name = state.engine.spreadsheet.names.cell_id_to_name(&abs_id);
    tauri::ipc::Response::new(name.into_bytes())
}

#[tauri::command(async)]
fn rename_cell(
    state: tauri::State<'_, Mutex<TonicState>>,
    cell_id: CellId,
    name: String,
) -> Result<(), String> {
    let mut state = state.lock().unwrap();
    let abs_id = cell_id.to_absolute();
    state
        .engine
        .spreadsheet
        .names
        .create_cell_name(&name, &abs_id)
        .ok_or_else(|| format!("Name '{}' is not available", name))
}

#[tauri::command(async)]
fn get_editor_value_for_cells(
    state: tauri::State<'_, Mutex<TonicState>>,
    cells: Vec<CellId>,
) -> tauri::ipc::Response {
    let state = state.lock().unwrap();
    // Encode as: [count: u32 LE] then for each cell: [len: u32 LE][bytes]
    let mut buf = Vec::new();
    buf.extend_from_slice(&(cells.len() as u32).to_le_bytes());
    for cell_id in cells {
        let value = get_editor_value(&state, cell_id);
        let bytes = value.as_bytes();
        buf.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        buf.extend_from_slice(bytes);
    }
    tauri::ipc::Response::new(buf)
}

#[tauri::command(async)]
fn init_viewport(state: tauri::State<'_, Mutex<TonicState>>) {
    let mut state = state.lock().unwrap();
    state.last_viewport_buf.clear();
    state.last_viewport_range = (u32::MAX, u32::MAX, u32::MAX, u32::MAX);
}

#[tauri::command(async)]
fn get_cells_in_viewport(
    state: tauri::State<'_, Mutex<TonicState>>,
    request: tauri::ipc::Request<'_>,
) -> tauri::ipc::Response {
    let headers = request.headers();
    let parse_header =
        |name: &str| -> Option<u32> { headers.get(name)?.to_str().ok()?.parse::<u32>().ok() };
    let Some(row_start) = parse_header("row-start") else {
        return tauri::ipc::Response::new(Vec::new());
    };
    let Some(row_end) = parse_header("row-end") else {
        return tauri::ipc::Response::new(Vec::new());
    };
    let Some(col_start) = parse_header("col-start") else {
        return tauri::ipc::Response::new(Vec::new());
    };
    let Some(col_end) = parse_header("col-end") else {
        return tauri::ipc::Response::new(Vec::new());
    };

    let state = &mut *state.lock().unwrap();
    let buf = &mut state.current_buf;
    let spreadsheet = &state.engine.spreadsheet;

    buf.clear();
    for row in row_start..=row_end {
        for col in col_start..=col_end {
            let id = AbsoluteCellId {
                sheet_id: 0,
                row,
                col,
            };
            let content = spreadsheet.get_projected_content(&id);
            encode_cell(buf, row, col, content);
        }
    }

    let range = (row_start, row_end, col_start, col_end);
    let viewport_changed = range != state.last_viewport_range;
    state.last_viewport_range = range;

    // return nothing if viewport range didn't change and
    // buffer that was sent previously is the same as the new buffer (no cell was updated in the current viewport)
    if !viewport_changed && state.current_buf == state.last_viewport_buf {
        return tauri::ipc::Response::new(Vec::new());
    }
    std::mem::swap(&mut state.current_buf, &mut state.last_viewport_buf);
    tauri::ipc::Response::new(state.last_viewport_buf.clone())
}

#[tauri::command(async)]
fn enter_input(
    app: AppHandle,
    cell_id: CellId,
    user_input: &str,
    state: tauri::State<'_, Mutex<TonicState>>,
) {
    let mut state = state.lock().unwrap();
    let t = state.engine.start_batch();
    state
        .engine
        .parse_and_insert_string(&t, cell_id.to_absolute(), user_input);
    state.engine.end_batch(t);
    emit_save_status(&app, &state);
}

#[tauri::command(async)]
fn delete_cells(app: AppHandle, state: tauri::State<'_, Mutex<TonicState>>, cells: Vec<CellId>) {
    let timer = std::time::Instant::now();
    let mut state = state.lock().unwrap();
    let t = state.engine.start_batch();
    for cell_id in cells {
        state.engine.delete(&t, cell_id.to_absolute());
    }
    state.engine.end_batch(t);
    debug!("Delete took: {:?}", timer.elapsed());
    emit_save_status(&app, &state);
}

#[tauri::command(async)]
fn fill_cells(
    app: AppHandle,
    state: tauri::State<'_, Mutex<TonicState>>,
    sources: Vec<CellId>,
    dests: Vec<CellId>,
    orig_min_row: u32,
    orig_max_row: u32,
    orig_min_col: u32,
    orig_max_col: u32,
) {
    if sources.len() != dests.len() {
        return;
    }

    let timer = std::time::Instant::now();
    let mut state = state.lock().unwrap();

    let get_num = |state: &TonicState, row: u32, col: u32| -> Option<Decimal> {
        let id = AbsoluteCellId {
            sheet_id: 0,
            row,
            col,
        };
        let content = state.engine.spreadsheet.get_content(&id)?;
        if content.defined_by_formula.is_some() {
            return None;
        }
        match &content.val {
            CellValue::Number(n) => Some(*n),
            _ => None,
        }
    };

    // compute per-column row-step for extrapolation
    // each column independently checks if it has a numeric linear pattern
    let orig_height = orig_max_row - orig_min_row + 1;
    let orig_width = orig_max_col - orig_min_col + 1;
    let mut col_row_steps: Vec<Option<(Decimal, Decimal)>> =
        Vec::with_capacity(orig_width as usize);
    for c in orig_min_col..=orig_max_col {
        if orig_height >= 2 {
            match (
                get_num(&state, orig_min_row, c),
                get_num(&state, orig_min_row + 1, c),
            ) {
                (Some(a), Some(b)) => col_row_steps.push(Some((a, b - a))),
                _ => col_row_steps.push(None),
            }
        } else if let Some(v) = get_num(&state, orig_min_row, c) {
            // single row: value is known but no step
            col_row_steps.push(Some((v, Decimal::ZERO)));
        } else {
            col_row_steps.push(None);
        }
    }

    // compute per-row col-step for extrapolation
    let mut row_col_steps: Vec<Option<(Decimal, Decimal)>> =
        Vec::with_capacity(orig_height as usize);
    for r in orig_min_row..=orig_max_row {
        if orig_width >= 2 {
            match (
                get_num(&state, r, orig_min_col),
                get_num(&state, r, orig_min_col + 1),
            ) {
                (Some(a), Some(b)) => row_col_steps.push(Some((a, b - a))),
                _ => row_col_steps.push(None),
            }
        } else if let Some(v) = get_num(&state, r, orig_min_col) {
            row_col_steps.push(Some((v, Decimal::ZERO)));
        } else {
            row_col_steps.push(None);
        }
    }

    let t = state.engine.start_batch();
    for i in 0..dests.len() {
        let dest = dests[i];
        let source = sources[i];
        let source_content = state
            .engine
            .spreadsheet
            .get_content(&source.to_absolute())
            .cloned();

        if let Some(content) = &source_content {
            if let Some(formula_id) = content.defined_by_formula {
                state
                    .engine
                    .insert_shared_formula(&t, dest.to_absolute(), formula_id);
                continue;
            }
        }

        let col_idx = (source.col - orig_min_col) as usize;
        let row_idx = (source.row - orig_min_row) as usize;
        let row_offset = dest.row as i64 - source.row as i64;
        let col_offset = dest.col as i64 - source.col as i64;

        // per-column extrapolation (vertical fill) + per-row extrapolation (horizontal fill)
        let mut extrapolated = None;
        if row_offset != 0 {
            if let Some((first_val, step)) = col_row_steps.get(col_idx).copied().flatten() {
                extrapolated = Some(first_val + step * Decimal::from(dest.row - orig_min_row));
            }
        }
        if col_offset != 0 {
            if let Some((first_val, step)) = row_col_steps.get(row_idx).copied().flatten() {
                extrapolated = Some(first_val + step * Decimal::from(dest.col - orig_min_col));
            }
        }

        if let Some(val) = extrapolated {
            state.engine.insert_number(&t, dest.to_absolute(), val);
        } else if let Some(content) = source_content {
            let text = content.val.to_string();
            state
                .engine
                .parse_and_insert_string(&t, dest.to_absolute(), &text);
        }
    }
    state.engine.end_batch(t);
    debug!("fill_cells took: {:?}", timer.elapsed());
    emit_save_status(&app, &state);
}

#[tauri::command(async)]
fn paste_values(
    app: AppHandle,
    state: tauri::State<'_, Mutex<TonicState>>,
    cells: Vec<(CellId, String)>,
) {
    let timer = std::time::Instant::now();
    let mut state = state.lock().unwrap();
    let t = state.engine.start_batch();
    for (cell_id, text) in cells {
        if text.is_empty() {
            state.engine.delete(&t, cell_id.to_absolute());
        } else {
            state
                .engine
                .parse_and_insert_string(&t, cell_id.to_absolute(), &text);
        }
    }
    state.engine.end_batch(t);
    debug!("Paste took: {:?}", timer.elapsed());
    emit_save_status(&app, &state);
}

#[tauri::command(async)]
fn undo_input(app: AppHandle, state: tauri::State<'_, Mutex<TonicState>>) -> Option<ChangeBounds> {
    let timer = std::time::Instant::now();
    let mut state = state.lock().unwrap();
    let bounds = state.engine.undo();
    debug!("Undo took: {:?}", timer.elapsed());
    emit_save_status(&app, &state);
    bounds
}

#[tauri::command(async)]
fn redo_input(app: AppHandle, state: tauri::State<'_, Mutex<TonicState>>) -> Option<ChangeBounds> {
    let timer = std::time::Instant::now();
    let mut state = state.lock().unwrap();
    let bounds = state.engine.redo();
    debug!("Redo took: {:?}", timer.elapsed());
    emit_save_status(&app, &state);
    bounds
}

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn Error + 'static>> {
    app.manage(Mutex::new(TonicState::new()));
    Ok(())
}

fn update_file_info(state: &mut TonicState, path: &str) {
    let p = std::path::Path::new(path);
    state.file_name = p.file_name().map(|n| n.to_string_lossy().into_owned());
    state.file_path = Some(path.to_string());
}

#[tauri::command(async)]
fn save_file(
    app: AppHandle,
    state: tauri::State<'_, Mutex<TonicState>>,
    path: &str,
    ui_decorations_json: &str,
) -> Result<(), String> {
    let path = if path.ends_with(".tcs") {
        path.to_string()
    } else {
        format!("{}.tcs", path)
    };
    let mut state = state.lock().unwrap();
    if let Err(e) = state.engine.save_spreadsheet(&path, ui_decorations_json) {
        error!("Failed to save file '{}': {}", path, e);
        return Err(e.to_string());
    }
    update_file_info(&mut state, &path);
    emit_save_status(&app, &state);
    Ok(())
}

#[tauri::command(async)]
fn rename_current_file(
    app: AppHandle,
    state: tauri::State<'_, Mutex<TonicState>>,
    new_name: &str,
) -> Result<(), String> {
    let mut state = state.lock().unwrap();
    let Some(old_path) = state.file_path.clone() else {
        state.file_name = Some(new_name.to_string());
        return Ok(());
    };
    let old = std::path::Path::new(&old_path);
    let new_path = old.with_file_name(new_name);
    let new_path_str = new_path.to_string_lossy().to_string();
    file_api::rename_file(&old_path, &new_path_str).map_err(|e| {
        error!(
            "Failed to rename '{}' to '{}': {}",
            old_path, new_path_str, e
        );
        e.to_string()
    })?;
    update_file_info(&mut state, &new_path_str);
    emit_save_status(&app, &state);
    Ok(())
}

#[tauri::command(async)]
fn open_file(
    app: AppHandle,
    state: tauri::State<'_, Mutex<TonicState>>,
    path: &str,
) -> Result<String, String> {
    let mut state = state.lock().unwrap();
    let decorations = state.engine.open_spreadsheet(path).map_err(|e| {
        error!("Failed to open file '{}': {}", path, e);
        e.to_string()
    })?;
    state.last_viewport_buf.clear();
    state.last_viewport_range = (u32::MAX, u32::MAX, u32::MAX, u32::MAX);
    update_file_info(&mut state, path);
    emit_save_status(&app, &state);
    Ok(decorations)
}

#[tauri::command(async)]
fn new_file(app: AppHandle, state: tauri::State<'_, Mutex<TonicState>>) {
    let mut state = state.lock().unwrap();
    state.file_name = Some("Untitled.tcv".to_string());
    state.file_path = None;
    state.engine.create_empty_spreadsheet();
    emit_save_status(&app, &state);
}

#[tauri::command(async)]
fn get_file_info(state: tauri::State<'_, Mutex<TonicState>>) -> (Option<String>, Option<String>) {
    let state = state.lock().unwrap();
    (state.file_name.clone(), state.file_path.clone())
}

#[tauri::command(async)]
fn create_table(
    state: tauri::State<'_, Mutex<TonicState>>,
    table_name: String,
    first_header: GridCellId,
    last_header: GridCellId,
    body_start: GridCellId,
    body_end: GridCellId,
) -> Result<u32, String> {
    let mut state = state.lock().unwrap();
    let sp = &mut state.engine.spreadsheet;

    if first_header.row != last_header.row {
        return Err("Header must be on a single row".into());
    }
    if body_start.row != first_header.row + 1 {
        return Err("Body must start immediately below header".into());
    }
    if first_header.col != body_start.col || last_header.col != body_end.col {
        return Err("Body columns must align with header".into());
    }
    if body_end.row < body_start.row || body_end.col < body_start.col {
        return Err("Invalid body bounds".into());
    }
    if sp.names.table_names.contains_key(&table_name) {
        return Err(format!("Table '{}' already exists", table_name));
    }

    // todo: remove code duplication with post_table_cells_change_hook and filter methods

    // check overlap with existing tables
    for maybe_table in sp.tables.iter() {
        let Some(t) = maybe_table else { continue };
        let h_overlap =
            last_header.col >= t.first_header.col && first_header.col <= t.last_header.col;
        let v_overlap = body_end.row >= t.first_header.row && first_header.row <= t.body_end.row;
        if h_overlap && v_overlap {
            return Err("Table overlaps with existing table".into());
        }
    }

    let num_cols = (last_header.col - first_header.col + 1) as usize;
    let rows: Vec<u32> = (body_start.row..=body_end.row).collect();

    // collect unique values per column for filter options, tracking counts
    let mut next_id: u32 = 1; // 0 is reserved for the "(Blanks)" filter option
    let filter_options: Vec<BTreeMap<CellValue, ProjectionFilterOption>> = (0..num_cols)
        .map(|col_offset| {
            let col = body_start.col + col_offset as u32;
            let mut opts: BTreeMap<CellValue, ProjectionFilterOption> = BTreeMap::new();
            for &row in &rows {
                if let Some(val) = sp.sheets[0].get_value(&GridCellId { row, col }) {
                    opts.entry(val.clone())
                        .and_modify(|o| o.count += 1)
                        .or_insert_with(|| {
                            let id = next_id;
                            next_id += 1;
                            ProjectionFilterOption {
                                id,
                                selected: true,
                                count: 1,
                            }
                        });
                }
            }
            opts
        })
        .collect();

    let proj_id = sp.projections.insert(Projection {
        sheet_id: 0,
        projection_start: body_start,
        projection_end: body_end,
        projected_rows: rows,
        active: false,
        filter_options_per_column: filter_options,
        sorting_options_per_column: vec![
            ProjectionSortOption {
                selected: false,
                desc: false
            };
            num_cols
        ],
        hidden_rows_count: 0,
        filter_show_blanks: vec![true; num_cols],
        next_filter_option_id: next_id,
    });

    let id = sp.tables.insert(Table {
        sheet_id: 0,
        name: table_name.clone(),
        first_header,
        last_header,
        body_start,
        body_end,
        projection_id: proj_id,
    });

    sp.names.table_names.insert(table_name.clone(), id);
    sp.names.table_names_lookup.insert(id, table_name);
    Ok(id)
}

#[tauri::command(async)]
fn change_table_name(
    state: tauri::State<'_, Mutex<TonicState>>,
    old_name: String,
    new_name: String,
) -> Result<(), String> {
    let mut state = state.lock().unwrap();
    let sp = &mut state.engine.spreadsheet;

    let id = *sp
        .names
        .table_names
        .get(&old_name)
        .ok_or_else(|| format!("Table '{}' not found", old_name))?;

    if old_name == new_name {
        return Ok(());
    }
    if sp.names.table_names.contains_key(&new_name) {
        return Err(format!("Table '{}' already exists", new_name));
    }

    let table = sp
        .tables
        .get_mut(id)
        .ok_or_else(|| format!("Table '{}' not found", old_name))?;
    table.name = new_name.clone();

    sp.names.table_names.remove(&old_name);
    sp.names.table_names.insert(new_name.clone(), id);
    sp.names.table_names_lookup.insert(id, new_name);
    Ok(())
}

#[tauri::command(async)]
fn toggle_table_sort(
    app: AppHandle,
    state: tauri::State<'_, Mutex<TonicState>>,
    header: GridCellId,
    desc: bool,
) -> Result<(), String> {
    let timer = std::time::Instant::now();

    // todo: remove code duplication with other sort and filter commands

    let mut state = state.lock().unwrap();
    let sp = &state.engine.spreadsheet;

    let (table_id, table) = sp
        .find_table_by_header(&header)
        .ok_or("Column header is not part of any table")?;
    let proj_id = table.projection_id;
    let was_active = sp.projections.get(proj_id).map_or(false, |p| p.active);

    state.engine.sort_table_column(table_id, header.col, desc)?;

    let is_active = state
        .engine
        .spreadsheet
        .projections
        .get(proj_id)
        .map_or(false, |p| p.active);
    emit_table_projection_events(&app, table_id, was_active, is_active, None);

    debug!("toggle_table_sort took: {:?}", timer.elapsed());
    Ok(())
}

#[tauri::command(async)]
fn toggle_table_filter(
    app: AppHandle,
    state: tauri::State<'_, Mutex<TonicState>>,
    header: GridCellId,
    filter_option_id: u32,
) -> Result<(), String> {
    let timer = std::time::Instant::now();

    let mut state = state.lock().unwrap();
    let sp = &state.engine.spreadsheet;

    let (table_id, table) = sp
        .find_table_by_header(&header)
        .ok_or("Column header is not part of any table")?;
    let proj_id = table.projection_id;
    let was_active = sp.projections.get(proj_id).map_or(false, |p| p.active);

    let hidden = state
        .engine
        .filter_table_column(table_id, header.col, filter_option_id)?;

    let is_active = state
        .engine
        .spreadsheet
        .projections
        .get(proj_id)
        .map_or(false, |p| p.active);
    emit_table_projection_events(&app, table_id, was_active, is_active, Some(hidden));

    debug!("toggle_table_filter took: {:?}", timer.elapsed());
    Ok(())
}

#[tauri::command(async)]
fn select_all_table_filters(
    app: AppHandle,
    state: tauri::State<'_, Mutex<TonicState>>,
    header: GridCellId,
) -> Result<(), String> {
    let timer = std::time::Instant::now();

    let mut state = state.lock().unwrap();
    let sp = &state.engine.spreadsheet;

    let (table_id, table) = sp
        .find_table_by_header(&header)
        .ok_or("Column header is not part of any table")?;
    let proj_id = table.projection_id;
    let was_active = sp.projections.get(proj_id).map_or(false, |p| p.active);

    let hidden = state
        .engine
        .select_all_column_filters(table_id, header.col)?;

    let is_active = state
        .engine
        .spreadsheet
        .projections
        .get(proj_id)
        .map_or(false, |p| p.active);
    emit_table_projection_events(&app, table_id, was_active, is_active, Some(hidden));

    debug!("select_all_table_filters took: {:?}", timer.elapsed());
    Ok(())
}

#[tauri::command(async)]
fn clear_all_table_filters(
    app: AppHandle,
    state: tauri::State<'_, Mutex<TonicState>>,
    header: GridCellId,
) -> Result<(), String> {
    let timer = std::time::Instant::now();

    let mut state = state.lock().unwrap();
    let sp = &state.engine.spreadsheet;

    let (table_id, table) = sp
        .find_table_by_header(&header)
        .ok_or("Column header is not part of any table")?;
    let proj_id = table.projection_id;
    let was_active = sp.projections.get(proj_id).map_or(false, |p| p.active);

    let hidden = state
        .engine
        .clear_all_column_filters(table_id, header.col)?;

    let is_active = state
        .engine
        .spreadsheet
        .projections
        .get(proj_id)
        .map_or(false, |p| p.active);
    emit_table_projection_events(&app, table_id, was_active, is_active, Some(hidden));

    debug!("clear_all_table_filters took: {:?}", timer.elapsed());
    Ok(())
}

#[tauri::command(async)]
fn apply_table_projection(
    app: AppHandle,
    state: tauri::State<'_, Mutex<TonicState>>,
    table_name: String,
) -> Result<(), String> {
    let timer = std::time::Instant::now();
    let mut state = state.lock().unwrap();
    let sp = &state.engine.spreadsheet;

    let &table_id = sp
        .names
        .table_names
        .get(&table_name)
        .ok_or_else(|| format!("Table '{}' not found", table_name))?;
    let table = sp.tables.get(table_id).ok_or("Table not found")?;
    let proj_id = table.projection_id;
    let proj = sp.projections.get(proj_id).ok_or("Projection not found")?;
    if !proj.active {
        return Err("Projection is not active".into());
    }

    let sheet_id = table.sheet_id;
    let col_start = table.body_start.col;
    let col_end = table.body_end.col;
    let body_start_row = table.body_start.row;
    let body_end_row = table.body_end.row;
    let projected_rows = proj.projected_rows.clone();

    // collect values in projected order per column
    let values_per_col: Vec<Vec<Option<CellValue>>> = (col_start..=col_end)
        .map(|col| {
            projected_rows
                .iter()
                .map(|&row| {
                    sp.sheets[sheet_id as usize]
                        .get_value(&GridCellId { row, col })
                        .cloned()
                })
                .collect()
        })
        .collect();

    // write back sequentially using engine methods
    let t = state.engine.start_batch();
    for (col_offset, values) in values_per_col.into_iter().enumerate() {
        let col = col_start + col_offset as u32;
        for (i, val) in values.into_iter().enumerate() {
            let id = CellId {
                row: body_start_row + i as u32,
                col,
            }
            .to_absolute();
            match val {
                Some(v) => state.engine.insert_value(&t, id, v),
                None => state.engine.delete(&t, id),
            }
        }
        // delete remaining rows beyond projected length
        let kept = projected_rows.len() as u32;
        for row in (body_start_row + kept)..=body_end_row {
            state.engine.delete(&t, CellId { row, col }.to_absolute());
        }
    }

    // reset projection before end_batch so post_cell_changes_hook sees it inactive
    let proj = state
        .engine
        .spreadsheet
        .projections
        .get_mut(proj_id)
        .unwrap();
    let num_cols = (col_end - col_start + 1) as usize;
    proj.active = false;
    proj.projected_rows = (body_start_row..=body_end_row).collect();
    proj.hidden_rows_count = 0;
    proj.filter_show_blanks = vec![true; num_cols];
    for opt in &mut proj.sorting_options_per_column {
        opt.selected = false;
    }

    state.engine.end_batch(t);
    debug!("apply_table_projection took: {:?}", timer.elapsed());

    let _ = app.emit("disable-table-projection", table_id);
    emit_save_status(&app, &state);
    Ok(())
}

#[tauri::command(async)]
fn disable_table_projection(
    app: AppHandle,
    state: tauri::State<'_, Mutex<TonicState>>,
    table_name: String,
) -> Result<(), String> {
    let mut state = state.lock().unwrap();
    let sp = &mut state.engine.spreadsheet;

    let &table_id = sp
        .names
        .table_names
        .get(&table_name)
        .ok_or_else(|| format!("Table '{}' not found", table_name))?;
    let table = sp.tables.get(table_id).ok_or("Table not found")?;
    let proj_id = table.projection_id;
    let col_start = table.body_start.col;
    let col_end = table.body_end.col;
    let body_start_row = table.body_start.row;
    let body_end_row = table.body_end.row;

    let proj = sp
        .projections
        .get_mut(proj_id)
        .ok_or("Projection not found")?;
    let num_cols = (col_end - col_start + 1) as usize;
    proj.active = false;
    proj.projected_rows = (body_start_row..=body_end_row).collect();
    proj.hidden_rows_count = 0;
    proj.filter_show_blanks = vec![true; num_cols];
    for opt in &mut proj.sorting_options_per_column {
        opt.selected = false;
    }

    // reset all filter options to selected
    for col_opts in proj.filter_options_per_column.iter_mut() {
        for opt in col_opts.values_mut() {
            opt.selected = true;
        }
    }

    let _ = app.emit("disable-table-projection", table_id);
    Ok(())
}

#[derive(Serialize)]
struct FilterOptionResponse {
    id: u32,
    val: String,
    selected: bool,
}

#[tauri::command(async)]
fn get_filter_options_for_table_column(
    state: tauri::State<'_, Mutex<TonicState>>,
    header: GridCellId,
) -> Result<Vec<FilterOptionResponse>, String> {
    let timer = std::time::Instant::now();
    let state = state.lock().unwrap();
    let sp = &state.engine.spreadsheet;

    let (_, table) = sp
        .find_table_by_header(&header)
        .ok_or("Column header is not part of any table")?;
    let col_idx = (header.col - table.first_header.col) as usize;

    let proj = sp
        .projections
        .get(table.projection_id)
        .ok_or("Projection not found")?;
    if col_idx >= proj.filter_options_per_column.len() {
        return Err("Column index out of range".into());
    }

    // id 0 is always the "(Blanks)" option
    let mut result = vec![FilterOptionResponse {
        id: 0,
        val: "(Blanks)".into(),
        selected: proj.filter_show_blanks[col_idx],
    }];
    result.extend(
        proj.filter_options_per_column[col_idx]
            .iter()
            .map(|(val, o)| FilterOptionResponse {
                id: o.id,
                val: val.to_string(),
                selected: o.selected,
            }),
    );

    debug!(
        "get_filter_options_for_table_column took: {:?}",
        timer.elapsed()
    );

    Ok(result)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(
            tauri_plugin_log::Builder::new()
                .target(tauri_plugin_log::Target::new(
                    tauri_plugin_log::TargetKind::Webview,
                ))
                .build(),
        )
        .plugin(tauri_plugin_dialog::init())
        .setup(setup)
        .invoke_handler(tauri::generate_handler![
            init_viewport,
            enter_input,
            fill_cells,
            delete_cells,
            paste_values,
            get_cells_in_viewport,
            get_editor_value_for_cell,
            get_editor_value_for_cells,
            get_name_for_cell,
            rename_cell,
            save_file,
            open_file,
            new_file,
            get_file_info,
            rename_current_file,
            undo_input,
            redo_input,
            create_table,
            change_table_name,
            toggle_table_sort,
            toggle_table_filter,
            select_all_table_filters,
            clear_all_table_filters,
            get_filter_options_for_table_column,
            apply_table_projection,
            disable_table_projection,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
