// How backend works
//
// Backend exposes tarui commands to the frontend. Every command is not async.
// Frontend polls for cells that are in the current viewport (cells currently
// visible on screen) each 20ms or so (via get_cells_in_viewport command).
//
// When user modifies the spreadsheet, the main thread is blocked until the
// backend reacts to modification (inserts values, recomputes dependencies, etc)

use std::{error::Error, sync::Mutex};

use fastnum::D256;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_log::log::error;

use crate::engine_api::{ChangeBounds, Engine};
use crate::parser::shift_formula_refs;
use crate::storage::grid::{CellContent, CellValue, GridCellId};
use crate::storage::types::AbsoluteCellId;

mod engine;
mod engine_api;
mod file_api;
mod parser;
mod sheet;
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
    last_viewport_buf: Vec<u8>,
    last_viewport_range: (u32, u32, u32, u32),
    file_name: Option<String>,
    file_path: Option<String>,
}

impl TonicState {
    fn new() -> Self {
        Self {
            engine: Engine::new(),
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
    let gid = GridCellId {
        row: cell_id.row,
        col: cell_id.col,
    };
    let Some(content) = state.engine.spreadsheet.sheets[0].get_content(&gid) else {
        return String::new();
    };

    // If defined by formula, shift references (formula_string already includes '=')
    if let Some(formula_id) = content.defined_by_formula {
        if let Some(formula) = state.engine.spreadsheet.formulas.get(formula_id) {
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

    let mut state = state.lock().unwrap();
    let grid = &state.engine.spreadsheet.sheets[0];

    let mut buf = Vec::new();
    for row in row_start..=row_end {
        for col in col_start..=col_end {
            let gid = GridCellId { row, col };
            let content = grid.get_content(&gid);
            encode_cell(&mut buf, row, col, content);
        }
    }

    let range = (row_start, row_end, col_start, col_end);
    let viewport_changed = range != state.last_viewport_range;
    state.last_viewport_range = range;

    // return nothing if viewport range didn't change and
    // buffer that was sent previously is the same as the new buffer (no cell was updated in the current viewport)
    if !viewport_changed && buf == state.last_viewport_buf {
        return tauri::ipc::Response::new(Vec::new());
    }
    state.last_viewport_buf = buf.clone();
    tauri::ipc::Response::new(buf)
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
    let mut state = state.lock().unwrap();
    let t = state.engine.start_batch();
    for cell_id in cells {
        state.engine.delete(&t, cell_id.to_absolute());
    }
    state.engine.end_batch(t);
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

    let mut state = state.lock().unwrap();
    let grid = &state.engine.spreadsheet.sheets[0];

    let get_num = |row: u32, col: u32| -> Option<D256> {
        let gid = GridCellId { row, col };
        match &grid.get_content(&gid)?.val {
            CellValue::Number(n) => Some(*n),
            _ => None,
        }
    };

    // compute extrapolation steps
    let first_val = get_num(orig_min_row, orig_min_col);
    let row_step = if orig_max_row > orig_min_row {
        match (first_val, get_num(orig_min_row + 1, orig_min_col)) {
            (Some(a), Some(b)) => Some(b - a),
            _ => None,
        }
    } else {
        None
    };
    let col_step = if orig_max_col > orig_min_col {
        match (first_val, get_num(orig_min_row, orig_min_col + 1)) {
            (Some(a), Some(b)) => Some(b - a),
            _ => None,
        }
    } else {
        None
    };
    let cross_step = match (first_val, row_step, col_step) {
        (Some(fv), Some(rs), Some(cs)) => {
            get_num(orig_min_row + 1, orig_min_col + 1).map(|d| d - fv - rs - cs)
        }
        _ => None,
    };

    let can_extrapolate = first_val.is_some() && (row_step.is_some() || col_step.is_some());

    let t = state.engine.start_batch();
    for i in 0..dests.len() {
        let dest = dests[i];
        if can_extrapolate {
            let fv = first_val.unwrap();
            let dr = D256::from(dest.row.abs_diff(orig_min_row));
            let dc = D256::from(dest.col.abs_diff(orig_min_col));
            let dest_val = fv
                + row_step.unwrap_or(D256::ZERO) * dr
                + col_step.unwrap_or(D256::ZERO) * dc
                + cross_step.unwrap_or(D256::ZERO) * dr * dc;
            state.engine.insert_number(&t, dest.to_absolute(), dest_val);
        } else {
            let source = sources[i];
            let source_gid = GridCellId {
                row: source.row,
                col: source.col,
            };
            if let Some(content) = state.engine.spreadsheet.sheets[0]
                .get_content(&source_gid)
                .cloned()
            {
                if let Some(formula_id) = content.defined_by_formula {
                    state
                        .engine
                        .insert_shared_formula(&t, dest.to_absolute(), formula_id);
                } else {
                    let text = content.val.to_string();
                    state
                        .engine
                        .parse_and_insert_string(&t, dest.to_absolute(), &text);
                }
            }
        }
    }
    state.engine.end_batch(t);
    emit_save_status(&app, &state);
}

#[tauri::command(async)]
fn paste_values(
    app: AppHandle,
    state: tauri::State<'_, Mutex<TonicState>>,
    cells: Vec<(CellId, String)>,
) {
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
    emit_save_status(&app, &state);
}

#[tauri::command(async)]
fn undo_input(app: AppHandle, state: tauri::State<'_, Mutex<TonicState>>) -> Option<ChangeBounds> {
    let mut state = state.lock().unwrap();
    let bounds = state.engine.undo();
    emit_save_status(&app, &state);
    bounds
}

#[tauri::command(async)]
fn redo_input(app: AppHandle, state: tauri::State<'_, Mutex<TonicState>>) -> Option<ChangeBounds> {
    let mut state = state.lock().unwrap();
    let bounds = state.engine.redo();
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
) -> Result<(), String> {
    let path = if path.ends_with(".tcs") {
        path.to_string()
    } else {
        format!("{}.tcs", path)
    };
    let mut state = state.lock().unwrap();
    if let Err(e) = state.engine.save_spreadsheet(&path) {
        error!("Failed to save file '{}': {}", path, e);
        return Err(e.to_string());
    }
    update_file_info(&mut state, &path);
    emit_save_status(&app, &state);
    Ok(())
}

#[tauri::command(async)]
fn open_file(
    app: AppHandle,
    state: tauri::State<'_, Mutex<TonicState>>,
    path: &str,
) -> Result<(), String> {
    let mut state = state.lock().unwrap();
    state.engine.open_spreadsheet(path).map_err(|e| {
        error!("Failed to open file '{}': {}", path, e);
        e.to_string()
    })?;
    state.last_viewport_buf.clear();
    state.last_viewport_range = (u32::MAX, u32::MAX, u32::MAX, u32::MAX);
    update_file_info(&mut state, path);
    emit_save_status(&app, &state);
    Ok(())
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
            save_file,
            open_file,
            new_file,
            get_file_info,
            undo_input,
            redo_input,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
