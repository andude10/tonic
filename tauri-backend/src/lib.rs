use std::{
    error::Error,
    sync::{Mutex, MutexGuard},
};

use fastnum::D256;
use tauri::Manager;

use crate::{
    engine::eval_formula,
    parser::{
        lex_formula, lexer_errors_to_string, offset_refs, parse_formula,
        parse_formula_errors_to_string, FormulaState,
    },
    sheet::{Cell, CellId, CellValue, Spreadsheet},
};

mod engine;
mod file_api;
mod parser;
mod sheet;

struct TonicState {
    spreadsheet: Spreadsheet,
    last_viewport_buf: Vec<u8>,
    last_viewport_range: (u32, u32),
}

impl TonicState {
    fn new() -> Self {
        Self {
            spreadsheet: Spreadsheet::new(),
            last_viewport_buf: Vec::new(),
            last_viewport_range: (u32::MAX, u32::MAX),
        }
    }
}

/// Emit a backend timing measurement to the frontend dev panel.
// fn emit_timing(app: &AppHandle, name: &str, start: Instant, count: bool) {
//     let _ = app.emit(
//         "backend-timing",
//         (name, start.elapsed().as_secs_f64() * 1000.0, count),
//     );
// }

/// Encode a single cell into the binary buffer.
/// Format: [row: u32 LE][col: u32 LE][is_error: u8][display_len: u32 LE][display][entered_len: u32 LE][entered]
fn encode_cell(buf: &mut Vec<u8>, spreadsheet: &Spreadsheet, cell_id: CellId) {
    let (display, is_error) = match spreadsheet.sheets[0].get(&cell_id) {
        Some(Cell::SingleValue(v)) => (v.to_string(), false),
        Some(Cell::Formula { value, .. }) => (value.to_string(), false),
        Some(Cell::FormulaError { error }) => (error.clone(), true),
        None => (String::new(), false),
    };
    let entered_text = spreadsheet
        .user_input_raw_text
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
        let Some(user_input) = sp.user_input_raw_text.get(&cell_id) else {
            continue;
        };

        // if not entering formula, then just update value
        if !user_input.starts_with('=') {
            if let Ok(n) = user_input.parse::<D256>() {
                sp.sheets[0].insert(cell_id, Cell::SingleValue(CellValue::Number(n)));
            } else {
                let text = user_input.clone();
                sp.sheets[0].insert(cell_id, Cell::SingleValue(CellValue::Text(text)));
            }
            continue;
        }

        // start parsing formula
        let formula_text = &user_input[1..];

        let lex_output = lex_formula(formula_text);

        // report any errors happened during lexing
        if lex_output.has_errors() {
            let msg = lexer_errors_to_string(lex_output.errors());
            sp.sheets[0].insert(cell_id, Cell::FormulaError { error: msg });
            continue;
        }

        // return if empty
        let Some(tokens) = lex_output.output() else {
            continue;
        };

        let mut formula_state = FormulaState {
            sheet_names: &mut sp.sheet_names,
            cell_names: &mut sp.cell_names,
            user_function_names: &mut sp.user_function_names,
            expr_arena: Vec::new(),
        };
        let (parsed, parse_errs) = parse_formula(tokens, formula_text.len(), &mut formula_state);

        // report any errors happened during parsing formula (syntax, name not found)
        if !parse_errs.is_empty() {
            let msg = parse_formula_errors_to_string(&parse_errs);
            sp.sheets[0].insert(cell_id, Cell::FormulaError { error: msg });
            continue;
        }

        if let Some((expr, _root)) = parsed {
            // report any evaluation errors
            match eval_formula(&expr, sp) {
                Ok(value) => {
                    sp.sheets[0].insert(cell_id, Cell::Formula { expr, value });
                }
                Err(e) => {
                    let msg = format!("Eval Error: {:?}", e);
                    sp.sheets[0].insert(cell_id, Cell::FormulaError { error: msg });
                }
            }
        }
    }
}

/// Remove cells from sheets[0].
fn remove_cells(state: &mut MutexGuard<'_, TonicState>, cell_ids: &[CellId]) {
    for cell_id in cell_ids {
        state.spreadsheet.sheets[0].remove(cell_id);
    }
}

#[tauri::command]
fn init_viewport(state: tauri::State<'_, Mutex<TonicState>>) {
    let mut state = state.lock().unwrap();
    state.last_viewport_buf.clear();
    state.last_viewport_range = (u32::MAX, u32::MAX);
}

#[tauri::command]
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
    let sheet = &state.spreadsheet.sheets[0];

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

#[tauri::command]
fn enter_input(cell_id: CellId, user_input: &str, state: tauri::State<'_, Mutex<TonicState>>) {
    let mut state = state
        .lock()
        .expect("to able to lock spreadsheet in enter_input");

    // always save user input
    state
        .spreadsheet
        .user_input_raw_text
        .insert(cell_id, user_input.to_string());

    parse_and_insert_cells(&mut state, &[cell_id]);
}

#[tauri::command]
fn delete_cells(state: tauri::State<'_, Mutex<TonicState>>, cells: Vec<CellId>) {
    let mut state = state
        .lock()
        .expect("to able to lock spreadsheet in delete_cells");

    for cell_id in &cells {
        state.spreadsheet.user_input_raw_text.remove(cell_id);
    }
    remove_cells(&mut state, &cells);
}

#[tauri::command]
fn fill_cells(
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

    let mut cells_to_parse: Vec<CellId> = Vec::with_capacity(dests.len());

    let sheet = &state.spreadsheet.sheets[0];
    let get_num = |cell: &CellId| -> Option<D256> {
        match sheet.get(cell)? {
            Cell::SingleValue(CellValue::Number(n)) => Some(*n),
            Cell::Formula {
                value: CellValue::Number(n),
                ..
            } => Some(*n),
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
            state
                .spreadsheet
                .user_input_raw_text
                .insert(dest, dest_val.to_string());
            cells_to_parse.push(dest);
            continue;
        }

        let row_off = dest.row as i32 - source.row as i32;
        let col_off = dest.col as i32 - source.col as i32;

        // offset all refs in the raw text
        let raw = state
            .spreadsheet
            .user_input_raw_text
            .get(&source)
            .cloned()
            .unwrap_or_default();
        let adjusted_raw = if raw.starts_with('=') {
            format!("={}", offset_refs(&raw[1..], row_off, col_off))
        } else {
            raw.clone()
        };
        state
            .spreadsheet
            .user_input_raw_text
            .insert(dest, adjusted_raw);
        cells_to_parse.push(dest);
    }

    parse_and_insert_cells(&mut state, &cells_to_parse);
}

#[tauri::command]
fn paste_values(state: tauri::State<'_, Mutex<TonicState>>, cells: Vec<(CellId, String)>) {
    let mut state = state.lock().expect("to able to lock state in paste_values");

    let mut to_remove: Vec<CellId> = Vec::new();
    let mut to_parse: Vec<CellId> = Vec::new();

    for (cell_id, text) in cells {
        if text.is_empty() {
            state.spreadsheet.user_input_raw_text.remove(&cell_id);
            to_remove.push(cell_id);
        } else {
            state.spreadsheet.user_input_raw_text.insert(cell_id, text);
            to_parse.push(cell_id);
        }
    }

    remove_cells(&mut state, &to_remove);
    parse_and_insert_cells(&mut state, &to_parse);
}

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn Error + 'static>> {
    app.manage(Mutex::new(TonicState::new()));
    Ok(())
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
        .setup(setup)
        .invoke_handler(tauri::generate_handler![
            init_viewport,
            enter_input,
            fill_cells,
            delete_cells,
            paste_values,
            get_cells_in_viewport
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
