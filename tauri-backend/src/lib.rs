use std::{error::Error, sync::Mutex, time::Instant};

use fastnum::D256;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_log::log::debug;

use crate::{
    engine::eval_formula,
    parser::{
        lex_formula, lexer_errors_to_string, offset_relative_refs, parse_formula,
        parse_formula_errors_to_string,
    },
    sheet::{Cell, CellId, CellValue, Expr, ExprAtom, Spreadsheet},
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
fn emit_timing(app: &AppHandle, name: &str, start: Instant, count: bool) {
    let _ = app.emit(
        "backend-timing",
        (name, start.elapsed().as_secs_f64() * 1000.0, count),
    );
}

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

#[tauri::command]
fn init_viewport(state: tauri::State<'_, Mutex<TonicState>>) {
    let mut state = state.lock().unwrap();
    state.last_viewport_buf.clear();
    state.last_viewport_range = (u32::MAX, u32::MAX);
}

#[tauri::command]
fn get_cells_in_viewport(
    app: AppHandle,
    state: tauri::State<'_, Mutex<TonicState>>,
    request: tauri::ipc::Request<'_>,
) -> tauri::ipc::Response {
    let get_cells_in_viewport_time = Instant::now();

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
fn enter_input(app: AppHandle, cell_id: CellId, user_input: &str) {
    let state = app.state::<Mutex<TonicState>>();
    let mut state = state
        .lock()
        .expect("to able to lock spreadsheet in enter_input");

    // always save user input
    state
        .spreadsheet
        .user_input_raw_text
        .insert(cell_id, user_input.to_string());

    // if not entering formula, then just update value
    if !user_input.starts_with('=') {
        let insert_time = Instant::now();
        if let Ok(n) = user_input.parse::<D256>() {
            state.spreadsheet.sheets[0].insert(cell_id, Cell::SingleValue(CellValue::Number(n)));
            debug!(
                "User entered number ({:?}): \"{}\", to {}",
                insert_time.elapsed(),
                n,
                cell_id
            );
        } else {
            state.spreadsheet.sheets[0].insert(
                cell_id,
                Cell::SingleValue(CellValue::Text(user_input.to_string())),
            );
            debug!(
                "User entered text ({:?}): \"{}\", to {}",
                insert_time.elapsed(),
                user_input,
                cell_id
            );
        }
        emit_timing(&app, "rs_insert_time", insert_time, false);
        return;
    }

    // start parsing formula
    let formula_text = &user_input[1..];

    let lex_time = Instant::now();
    let lex_output = lex_formula(formula_text);
    debug!("Lexer elapsed: {:?}", lex_time.elapsed());

    // report any errors happened during lexing
    if lex_output.has_errors() {
        let msg = lexer_errors_to_string(lex_output.errors());
        state.spreadsheet.sheets[0].insert(cell_id, Cell::FormulaError { error: msg });
        return;
    }

    // return if empty
    let Some(tokens) = lex_output.output() else {
        return;
    };

    let parse_time = Instant::now();
    let (parsed, parse_errs) = parse_formula(tokens, formula_text.len(), &mut state.spreadsheet);
    debug!("Parser elapsed: {:?}", parse_time.elapsed());

    // report any errors happened during parsing formula (syntax, not found name)
    if !parse_errs.is_empty() {
        let msg = parse_formula_errors_to_string(&parse_errs);
        state.spreadsheet.sheets[0].insert(cell_id, Cell::FormulaError { error: msg });
        return;
    }

    debug!("parsed formula: {:?}", parsed);

    if let Some((exprs, _root)) = parsed {
        // report any evaluation errors
        let eval_time = Instant::now();
        match eval_formula(&exprs, &state.spreadsheet) {
            Ok(value) => {
                state.spreadsheet.sheets[0].insert(cell_id, Cell::Formula { expr: exprs, value });
            }
            Err(e) => {
                let msg = format!("Eval Error: {:?}", e);
                state.spreadsheet.sheets[0].insert(cell_id, Cell::FormulaError { error: msg });
            }
        }
        emit_timing(&app, "rs_eval_time", eval_time, false);
        debug!("Eval elapsed: {:?}", eval_time.elapsed());
    }
}

#[tauri::command]
fn delete_cells(state: tauri::State<'_, Mutex<TonicState>>, cells: Vec<CellId>) {
    let mut state = state
        .lock()
        .expect("to able to lock spreadsheet in delete_cells");

    for cell_id in &cells {
        state.spreadsheet.user_input_raw_text.remove(cell_id);
        state.spreadsheet.sheets[0].remove(cell_id);
    }
}

#[tauri::command]
fn fill_cells(
    state: tauri::State<'_, Mutex<TonicState>>,
    sources: Vec<CellId>,
    dests: Vec<CellId>,
    before_sources: Option<Vec<Option<CellId>>>,
) {
    if sources.len() != dests.len() {
        return;
    }
    if let Some(ref bs) = before_sources {
        if bs.len() != dests.len() {
            return;
        }
    }

    let mut state = state
        .lock()
        .expect("to able to lock spreadsheet in fill_cells");

    for i in 0..dests.len() {
        let source = sources[i];
        let dest = dests[i];
        let before_source = before_sources
            .as_ref()
            .and_then(|v| v.get(i))
            .copied()
            .flatten();

        // numeric extrapolation: dest = source + (source - before_source)
        if let Some(before) = before_source {
            if let (
                Some(Cell::SingleValue(CellValue::Number(src_val))),
                Some(Cell::SingleValue(CellValue::Number(before_val))),
            ) = (
                state.spreadsheet.sheets[0].get(&source),
                state.spreadsheet.sheets[0].get(&before),
            ) {
                let (src_val, before_val) = (*src_val, *before_val);
                let dest_val = src_val + (src_val - before_val);
                let display = dest_val.to_string();
                state
                    .spreadsheet
                    .user_input_raw_text
                    .insert(dest, display.clone());
                state.spreadsheet.sheets[0]
                    .insert(dest, Cell::SingleValue(CellValue::Number(dest_val)));
                continue;
            }
        }

        let source_cell = state.spreadsheet.sheets[0].get(&source);

        match source_cell {
            Some(Cell::Formula { expr, value: _ }) => {
                let row_off = dest.row as i32 - source.row as i32;
                let col_off = dest.col as i32 - source.col as i32;

                // clone and offset relative refs in the AST
                let mut new_exprs: Vec<Expr> = expr.clone();
                for e in &mut new_exprs {
                    if let Expr::Atom(atom) = e {
                        match atom {
                            ExprAtom::RelativeCellRef(_, ref mut cell) => {
                                cell.row = (cell.row as i32 + row_off).max(0) as u32;
                                cell.col = (cell.col as i32 + col_off).max(0) as u32;
                            }
                            ExprAtom::RelativeCellRange(_, ref mut range) => {
                                for cell in [&mut range.start, &mut range.end] {
                                    cell.row = (cell.row as i32 + row_off).max(0) as u32;
                                    cell.col = (cell.col as i32 + col_off).max(0) as u32;
                                }
                            }
                            _ => {}
                        }
                    }
                }

                // offset relative refs in the raw text for display
                let raw = state
                    .spreadsheet
                    .user_input_raw_text
                    .get(&source)
                    .cloned()
                    .unwrap_or_default();
                let adjusted_raw = if raw.starts_with('=') {
                    format!("={}", offset_relative_refs(&raw[1..], row_off, col_off))
                } else {
                    raw
                };
                state
                    .spreadsheet
                    .user_input_raw_text
                    .insert(dest, adjusted_raw);

                // eval the offset AST
                match eval_formula(&new_exprs, &state.spreadsheet) {
                    Ok(value) => {
                        state.spreadsheet.sheets[0].insert(
                            dest,
                            Cell::Formula {
                                expr: new_exprs,
                                value,
                            },
                        );
                    }
                    Err(e) => {
                        let msg = format!("Eval Error: {:?}", e);
                        state.spreadsheet.sheets[0].insert(dest, Cell::FormulaError { error: msg });
                    }
                }
            }
            Some(Cell::SingleValue(value)) => {
                let value = value.clone();
                let raw = state
                    .spreadsheet
                    .user_input_raw_text
                    .get(&source)
                    .cloned()
                    .unwrap_or_default();
                state.spreadsheet.user_input_raw_text.insert(dest, raw);
                state.spreadsheet.sheets[0].insert(dest, Cell::SingleValue(value));
            }
            Some(Cell::FormulaError { .. }) | None => {}
        }
    }
}

#[tauri::command]
fn paste_values(
    _app: AppHandle,
    state: tauri::State<'_, Mutex<TonicState>>,
    cells: Vec<(CellId, String)>,
) {
    let mut state = state.lock().expect("to able to lock state in paste_values");

    for (cell_id, text) in cells {
        if text.is_empty() {
            state.spreadsheet.user_input_raw_text.remove(&cell_id);
            state.spreadsheet.sheets[0].remove(&cell_id);
            continue;
        }

        state
            .spreadsheet
            .user_input_raw_text
            .insert(cell_id, text.clone());

        if let Ok(n) = text.parse::<D256>() {
            state.spreadsheet.sheets[0].insert(cell_id, Cell::SingleValue(CellValue::Number(n)));
        } else {
            state.spreadsheet.sheets[0].insert(cell_id, Cell::SingleValue(CellValue::Text(text)));
        }
    }
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
