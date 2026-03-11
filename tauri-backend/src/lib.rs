use std::{
    error::Error,
    sync::{Mutex, MutexGuard},
    time::Instant,
};

use fastnum::D256;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_log::log::{debug, error, info};

use crate::{
    engine::eval,
    parser::{
        lex_formula, lexer_errors_to_string, offset_refs, parse_formula,
        parse_formula_errors_to_string, FormulaState,
    },
    sheet::{create_cell_with_formula_error, Cell, CellId, CellValue, Spreadsheet},
};

mod engine;
mod file_api;
mod parser;
mod sheet;
mod sheet_store;

enum InputLogEntry {
    Update {
        id: u64,
        cell_ids: Vec<CellId>,
        old_user_strings: Vec<String>,
        new_user_strings: Vec<String>,
    },
    Delete {
        id: u64,
        cell_ids: Vec<CellId>,
        old_user_strings: Vec<String>,
    },
}

struct TonicState {
    spreadsheet: Spreadsheet,

    input_log: Vec<InputLogEntry>,

    // the index for next log entry
    // (so the last log entry that was written is at next_input_log_position - 1)
    next_input_log_position: usize,
    next_log_entry_id: u64,

    // The id of the last log entry at save time, or None when file is not saved.
    // Equals Some(0) when just opened the file from disk
    saved_log_entry_id: Option<u64>,

    last_viewport_buf: Vec<u8>,
    last_viewport_range: (u32, u32),
    file_name: Option<String>,
    file_path: Option<String>,
}

impl TonicState {
    fn new() -> Self {
        Self {
            spreadsheet: Spreadsheet::new(),
            input_log: Vec::new(),
            next_input_log_position: 0,
            next_log_entry_id: 1, // zero should be already saved
            saved_log_entry_id: None,
            last_viewport_buf: Vec::new(),
            last_viewport_range: (u32::MAX, u32::MAX),
            file_name: None,
            file_path: None,
        }
    }
}

fn emit_save_status(app: &AppHandle, state: &TonicState) {
    let current_id = if state.next_input_log_position == 0 {
        Some(0)
    } else {
        match state.input_log[state.next_input_log_position - 1] {
            InputLogEntry::Update { id, .. } | InputLogEntry::Delete { id, .. } => Some(id),
        }
    };
    let is_saved = current_id == state.saved_log_entry_id;
    let _ = app.emit("save-status", is_saved);
}

/// Truncate any redo history and push an entry to the log.
fn push_input_to_log(state: &mut TonicState, mut entry: InputLogEntry) {
    state.input_log.truncate(state.next_input_log_position);
    let id = state.next_log_entry_id;
    state.next_log_entry_id += 1;

    // set id of the new input log
    match &mut entry {
        InputLogEntry::Update {
            id: ref mut eid, ..
        } => *eid = id,
        InputLogEntry::Delete {
            id: ref mut eid, ..
        } => *eid = id,
    }

    state.input_log.push(entry);
    state.next_input_log_position = state.input_log.len();
}

/// Set user_input_raw_text for each cell and log the change.
fn update_user_strings(
    state: &mut MutexGuard<'_, TonicState>,
    cell_ids: &[CellId],
    new_strings: Vec<String>,
) {
    let old_strings: Vec<String> = cell_ids
        .iter()
        .map(|id| {
            state
                .spreadsheet
                .user_strings
                .get(id)
                .cloned()
                .unwrap_or_default()
        })
        .collect();
    for (id, s) in cell_ids.iter().zip(new_strings.iter()) {
        state.spreadsheet.user_strings.insert(*id, s.clone());
    }
    push_input_to_log(
        state,
        InputLogEntry::Update {
            id: 0,
            cell_ids: cell_ids.to_vec(),
            old_user_strings: old_strings,
            new_user_strings: new_strings,
        },
    );
}

/// Remove user_input_raw_text for each cell and log the change.
fn remove_user_strings(state: &mut MutexGuard<'_, TonicState>, cell_ids: &[CellId]) {
    let old_strings: Vec<String> = cell_ids
        .iter()
        .map(|id| {
            state
                .spreadsheet
                .user_strings
                .get(id)
                .cloned()
                .unwrap_or_default()
        })
        .collect();
    for id in cell_ids {
        state.spreadsheet.user_strings.remove(id);
    }
    push_input_to_log(
        state,
        InputLogEntry::Delete {
            id: 0,
            cell_ids: cell_ids.to_vec(),
            old_user_strings: old_strings,
        },
    );
}

/// Emit a backend timing measurement to the frontend dev panel.
// fn emit_timing(app: &AppHandle, name: &str, start: Instant, count: bool) {
//     let _ = app.emit(
//         "backend-timing",
//         (name, start.elapsed().as_secs_f64() * 1000.0, count),
//     );
// }

// How backend works
//
// Backend exposes tarui commands to the frontend. Every command is not async.
// Frontend polls for cells that are in the current viewport (cells currently
// visible on screen) each 20ms or so (via get_cells_in_viewport command).
//
// When user modifies the spreadsheet, the main thread is blocked until the
// backend reacts to modification (inserts values, recomputes dependencies, etc)
//

/// Encode a single cell into the binary buffer.
/// Format: [row: u32 LE][col: u32 LE][is_error: u8][display_len: u32 LE][display][entered_len: u32 LE][entered]
fn encode_cell(buf: &mut Vec<u8>, spreadsheet: &Spreadsheet, cell_id: CellId) {
    let (display, is_error) = match spreadsheet.sheets[0].btree.get(&cell_id) {
        Some(Cell::SingleValue(v)) => (v.to_string(), false),
        Some(Cell::Formula {
            value: Some(value), ..
        }) => (value.to_string(), false),
        Some(Cell::Formula { value: None, .. }) => (String::new(), false),
        None => (String::new(), false),
    };
    let entered_text = spreadsheet
        .user_strings
        .get(&cell_id)
        .map(|s| s.as_bytes())
        .unwrap_or(b"");

    let display_bytes = display.as_bytes();

    buf.extend_from_slice(&cell_id.row.to_le_bytes());
    buf.extend_from_slice(&cell_id.col.to_le_bytes());
    buf.push(is_error as u8);
    buf.extend_from_slice(&(display_bytes.len() as u32).to_le_bytes());
    buf.extend_from_slice(display_bytes);
    buf.extend_from_slice(&(entered_text.len() as u32).to_le_bytes());
    buf.extend_from_slice(entered_text);
}

/// Parse user_input_raw_text for each given cell_id and insert the result into sheets[0].
fn parse_and_insert_cells(state: &mut MutexGuard<'_, TonicState>, cell_ids: &[CellId]) {
    let sp = &mut state.spreadsheet;

    for &cell_id in cell_ids {
        // todo: always remove cell_id from dependants

        let Some(user_input) = sp.user_strings.get(&cell_id) else {
            continue;
        };

        // if not entering formula, then just update value
        if !user_input.starts_with('=') {
            if let Ok(n) = user_input.parse::<D256>() {
                sp.sheets[0]
                    .btree
                    .insert(cell_id, Cell::SingleValue(CellValue::Number(n)));
            } else {
                let text = user_input.clone();
                sp.sheets[0]
                    .btree
                    .insert(cell_id, Cell::SingleValue(CellValue::Text(text)));
            }
            continue;
        }

        // start parsing formula
        let formula_text = &user_input[1..];

        let lex_output = lex_formula(formula_text);

        // report any errors happened during lexing
        if lex_output.has_errors() {
            let msg = lexer_errors_to_string(lex_output.errors());
            sp.sheets[0]
                .btree
                .insert(cell_id, create_cell_with_formula_error(msg));
            continue;
        }

        // return if empty
        let Some(tokens) = lex_output.output() else {
            continue;
        };

        let sheet = &mut sp.sheets[0];
        let mut formula_parser_state = FormulaState {
            cell_id,
            sheet_names: &mut sp.sheet_names,
            cell_names: &mut sp.cell_names,
            user_function_names: &mut sp.user_function_names,
            dependencies: &mut sheet.dependencies,
            dependents: &mut sheet.dependents,
            expr_arena: Vec::new(),
        };
        let (parsed, parse_errs) =
            parse_formula(tokens, formula_text.len(), &mut formula_parser_state);

        // report any errors happened during parsing formula (syntax, name not found)
        if !parse_errs.is_empty() {
            let msg = parse_formula_errors_to_string(&parse_errs);
            sp.sheets[0]
                .btree
                .insert(cell_id, create_cell_with_formula_error(msg));
            continue;
        }

        if let Some((expr, _root)) = parsed {
            sp.sheets[0].btree.insert(
                cell_id,
                Cell::Formula {
                    expr,
                    value: None,
                    prev_value: None,
                },
            );
        }
    }

    let eval_time = Instant::now();
    eval(cell_ids, sp);
    debug!("Eval took: {:?}", eval_time.elapsed());
}

// todo: remove in favor of parse_and_insert_cells
/// Remove cells from sheets[0].
fn remove_cells(state: &mut MutexGuard<'_, TonicState>, cell_ids: &[CellId]) {
    let sp = &mut state.spreadsheet;

    for cell_id in cell_ids {
        sp.sheets[0].btree.remove(cell_id);
    }

    let eval_time = Instant::now();
    eval(cell_ids, sp);
    debug!("Eval took: {:?}", eval_time.elapsed());
}

#[tauri::command(async)]
fn init_viewport(state: tauri::State<'_, Mutex<TonicState>>) {
    let mut state = state.lock().unwrap();
    state.last_viewport_buf.clear();
    state.last_viewport_range = (u32::MAX, u32::MAX);
}

// todo: maybe rename to "pull_cells"?
#[tauri::command(async)]
fn get_cells_in_viewport(
    state: tauri::State<'_, Mutex<TonicState>>,
    request: tauri::ipc::Request<'_>,
) -> tauri::ipc::Response {
    let headers = request.headers();
    let Some(row_start) = headers
        .get("row-start")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u32>().ok())
    else {
        return tauri::ipc::Response::new(Vec::new());
    };
    let Some(row_end) = headers
        .get("row-end")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u32>().ok())
    else {
        return tauri::ipc::Response::new(Vec::new());
    };

    let mut state = state.lock().unwrap();
    let sheet = &state.spreadsheet.sheets[0].btree;

    let mut buf = Vec::new();
    let lo = CellId {
        col: 0,
        row: row_start,
    };
    let hi = CellId {
        col: u32::MAX,
        row: row_end,
    };
    for (&cell_id, _) in sheet.range(lo..=hi) {
        if cell_id.row < row_start || cell_id.row > row_end {
            continue;
        }
        encode_cell(&mut buf, &state.spreadsheet, cell_id);
    }

    let range = (row_start, row_end);
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
    let mut state = state
        .lock()
        .expect("to able to lock spreadsheet in enter_input");
    update_user_strings(&mut state, &[cell_id], vec![user_input.to_string()]);
    parse_and_insert_cells(&mut state, &[cell_id]);
    emit_save_status(&app, &state);
}

#[tauri::command(async)]
fn delete_cells(app: AppHandle, state: tauri::State<'_, Mutex<TonicState>>, cells: Vec<CellId>) {
    let mut state = state
        .lock()
        .expect("to able to lock spreadsheet in delete_cells");
    remove_user_strings(&mut state, &cells);
    remove_cells(&mut state, &cells);
    emit_save_status(&app, &state);
}

// todo: simplify and rename to clone_cells
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

    let mut state = state
        .lock()
        .expect("to able to lock spreadsheet in fill_cells");

    let mut new_cell_ids: Vec<CellId> = Vec::with_capacity(dests.len());
    let mut new_strings: Vec<String> = Vec::with_capacity(dests.len());

    let sheet = &state.spreadsheet.sheets[0].btree;
    let get_num = |cell: &CellId| -> Option<D256> {
        match sheet.get(cell)? {
            Cell::SingleValue(CellValue::Number(n)) => Some(*n),
            _ => None,
        }
    };

    // perform numerical extrapolation
    // todo: double check math here

    // compute global steps from the first cells of the original range
    // dest(r,c) = first + row_step*dr + col_step*dc + cross_step*dr*dc
    let first = CellId {
        col: orig_min_col,
        row: orig_min_row,
    };
    let first_val = get_num(&first);
    let row_step = if orig_max_row > orig_min_row {
        let second = CellId {
            col: orig_min_col,
            row: orig_min_row + 1,
        };
        match (first_val, get_num(&second)) {
            (Some(a), Some(b)) => Some(b - a),
            _ => None,
        }
    } else {
        None
    };
    let col_step = if orig_max_col > orig_min_col {
        let second = CellId {
            col: orig_min_col + 1,
            row: orig_min_row,
        };
        match (first_val, get_num(&second)) {
            (Some(a), Some(b)) => Some(b - a),
            _ => None,
        }
    } else {
        None
    };
    let cross_step = match (first_val, row_step, col_step) {
        (Some(fv), Some(rs), Some(cs)) => {
            let diag = CellId {
                col: orig_min_col + 1,
                row: orig_min_row + 1,
            };
            get_num(&diag).map(|d| d - fv - rs - cs)
        }
        _ => None,
    };

    let can_extrapolate = first_val.is_some() && (row_step.is_some() || col_step.is_some());

    for i in 0..dests.len() {
        let source = sources[i];
        let dest = dests[i];

        if can_extrapolate {
            let fv = first_val.unwrap();
            let dr = D256::from(dest.row.abs_diff(orig_min_row));
            let dc = D256::from(dest.col.abs_diff(orig_min_col));
            let dest_val = fv
                + row_step.unwrap_or(D256::ZERO) * dr
                + col_step.unwrap_or(D256::ZERO) * dc
                + cross_step.unwrap_or(D256::ZERO) * dr * dc;
            new_cell_ids.push(dest);
            new_strings.push(dest_val.to_string());
            continue;
        }

        let row_off = dest.row as i32 - source.row as i32;
        let col_off = dest.col as i32 - source.col as i32;

        // offset all refs in the raw text
        let raw = state
            .spreadsheet
            .user_strings
            .get(&source)
            .cloned()
            .unwrap_or_default();
        let adjusted_raw = if raw.starts_with('=') {
            format!("={}", offset_refs(&raw[1..], row_off, col_off))
        } else {
            raw.clone()
        };
        new_cell_ids.push(dest);
        new_strings.push(adjusted_raw);
    }

    update_user_strings(&mut state, &new_cell_ids, new_strings);
    parse_and_insert_cells(&mut state, &new_cell_ids);
    emit_save_status(&app, &state);
}

#[tauri::command(async)]
fn paste_values(
    app: AppHandle,
    state: tauri::State<'_, Mutex<TonicState>>,
    cells: Vec<(CellId, String)>,
) {
    let mut state = state.lock().expect("to able to lock state in paste_values");

    // if pasted new value to a cell, then update cell's value.
    // If pasted nothing, then remove cell

    let mut ids_to_remove: Vec<CellId> = Vec::new();
    let mut ids_to_update: Vec<CellId> = Vec::new();
    let mut strings_to_update: Vec<String> = Vec::new();

    for (cell_id, text) in cells {
        if text.is_empty() {
            ids_to_remove.push(cell_id);
        } else {
            ids_to_update.push(cell_id);
            strings_to_update.push(text);
        }
    }

    if !ids_to_remove.is_empty() {
        remove_user_strings(&mut state, &ids_to_remove);
        remove_cells(&mut state, &ids_to_remove);
    }
    if !ids_to_update.is_empty() {
        update_user_strings(&mut state, &ids_to_update, strings_to_update);
        parse_and_insert_cells(&mut state, &ids_to_update);
    }
    emit_save_status(&app, &state);
}

// todo: undo_input and redo_input can be quite slow in theory
// remove (or change) return value of undo_input and redo_input?
// used by frotned to change focus after undo/redo

#[tauri::command(async)]
fn undo_input(app: AppHandle, state: tauri::State<'_, Mutex<TonicState>>) -> Vec<CellId> {
    let undo_time = Instant::now();
    let mut state = state.lock().unwrap();
    if state.next_input_log_position == 0 {
        return vec![];
    }
    state.next_input_log_position -= 1;
    let idx = state.next_input_log_position;
    let (cell_ids, old) = match &state.input_log[idx] {
        InputLogEntry::Update {
            cell_ids,
            old_user_strings,
            ..
        }
        | InputLogEntry::Delete {
            cell_ids,
            old_user_strings,
            ..
        } => (cell_ids.clone(), old_user_strings.clone()),
    };
    for (id, s) in cell_ids.iter().zip(old.into_iter()) {
        state.spreadsheet.user_strings.insert(*id, s);
    }
    parse_and_insert_cells(&mut state, &cell_ids);
    emit_save_status(&app, &state);
    debug!("Undo took {:?}", undo_time.elapsed());
    cell_ids
}

#[tauri::command(async)]
fn redo_input(app: AppHandle, state: tauri::State<'_, Mutex<TonicState>>) -> Vec<CellId> {
    let redo_time = Instant::now();
    let mut state = state.lock().unwrap();
    if state.next_input_log_position >= state.input_log.len() {
        return vec![];
    }
    let idx = state.next_input_log_position;
    state.next_input_log_position += 1;
    match &state.input_log[idx] {
        InputLogEntry::Update {
            cell_ids,
            new_user_strings,
            ..
        } => {
            let cell_ids = cell_ids.clone();
            let new = new_user_strings.clone();
            for (id, s) in cell_ids.iter().zip(new.into_iter()) {
                state.spreadsheet.user_strings.insert(*id, s);
            }
            parse_and_insert_cells(&mut state, &cell_ids);
            emit_save_status(&app, &state);
            debug!("Redo took {:?}", redo_time.elapsed());
            cell_ids
        }
        InputLogEntry::Delete { cell_ids, .. } => {
            let cell_ids = cell_ids.clone();
            for id in &cell_ids {
                state.spreadsheet.user_strings.remove(id);
            }
            remove_cells(&mut state, &cell_ids);
            emit_save_status(&app, &state);
            debug!("Redo took {:?}", redo_time.elapsed());
            cell_ids
        }
    }
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
    if let Err(e) = file_api::save(&state.spreadsheet, &path) {
        error!("Failed to save file '{}': {}", path, e);
        return Err(e.to_string());
    }
    update_file_info(&mut state, &path);
    state.saved_log_entry_id = if state.next_input_log_position == 0 {
        Some(0)
    } else {
        match state.input_log[state.next_input_log_position - 1] {
            InputLogEntry::Update { id, .. } | InputLogEntry::Delete { id, .. } => Some(id),
        }
    };
    emit_save_status(&app, &state);
    Ok(())
}

#[tauri::command(async)]
fn open_file(
    app: AppHandle,
    state: tauri::State<'_, Mutex<TonicState>>,
    path: &str,
) -> Result<(), String> {
    let spreadsheet = file_api::load(path).map_err(|e| {
        error!("Failed to open file '{}': {}", path, e);
        e.to_string()
    })?;
    let mut state = state.lock().unwrap();
    state.spreadsheet = spreadsheet;
    state.last_viewport_buf.clear();
    state.last_viewport_range = (u32::MAX, u32::MAX);
    state.input_log.clear();
    state.next_input_log_position = 0;
    state.next_log_entry_id = 1;
    state.saved_log_entry_id = Some(0);
    update_file_info(&mut state, path);
    emit_save_status(&app, &state);
    Ok(())
}

#[tauri::command(async)]
fn new_file(app: AppHandle, state: tauri::State<'_, Mutex<TonicState>>) {
    let mut state = state.lock().unwrap();
    state.file_name = Some("Untitled.tcv".to_string());
    state.file_path = None;
    state.spreadsheet = Spreadsheet::new();
    state.input_log.clear();
    state.next_input_log_position = 0;
    state.next_log_entry_id = 1;
    state.saved_log_entry_id = None;
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
