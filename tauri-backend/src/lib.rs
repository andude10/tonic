use std::{error::Error, sync::Mutex, time::Instant};

use ::serde::{Deserialize, Serialize};
use fastnum::D256;
use tauri::{ipc::Channel, AppHandle, Manager};
use tauri_plugin_log::log::{debug, error};

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
    spreadsheet: Mutex<Spreadsheet>,
}

impl TonicState {
    pub fn new() -> Self {
        Self {
            spreadsheet: Mutex::new(Spreadsheet::new()),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RequestWindow {
    start: CellId,
    end: CellId,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WindowCell {
    cell_id: CellId,
    display: String,
    entered_text: String,
}

#[derive(Clone, Serialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "event",
    content = "data"
)]
enum ComputeFormulaEvent<'a> {
    ParseErr {
        cell_id: CellId,
        message: &'a str,
    },
    Finished {
        cell_id: CellId,
        display_string: &'a str,
    },
}

fn send_result(channel: &Channel<ComputeFormulaEvent>, cell_id: CellId, display_string: &str) {
    if let Err(e) = channel.send(ComputeFormulaEvent::Finished {
        cell_id,
        display_string,
    }) {
        error!("failed to send formula result for {cell_id:?}: {e}");
    }
}

fn send_error(channel: &Channel<ComputeFormulaEvent>, cell_id: CellId, message: &str) {
    if let Err(e) = channel.send(ComputeFormulaEvent::ParseErr { cell_id, message }) {
        error!("failed to send formula error for {cell_id:?}: {e}");
    }
}

#[tauri::command]
fn get_spreadsheet_window(app: AppHandle, window: RequestWindow) -> Vec<WindowCell> {
    let state = app.state::<TonicState>();
    let spreadsheet = state
        .spreadsheet
        .lock()
        .expect("to able to lock spreadsheet in get_spreadsheet_window");

    let sheet = &spreadsheet.sheets[0];
    let start = CellId {
        col: window.start.col,
        row: window.start.row,
    };
    let end = CellId {
        col: window.end.col,
        row: window.end.row,
    };

    let mut result = Vec::new();
    for (&cell_id, cell) in sheet.range(start..=end) {
        let display = match cell {
            Cell::SingleValue(v) => v.to_string(),
            Cell::Formula { value, .. } => value.to_string(),
        };
        let entered_text = spreadsheet
            .user_input_raw_text
            .get(&cell_id)
            .cloned()
            .unwrap_or_default();
        result.push(WindowCell {
            cell_id,
            display,
            entered_text,
        });
    }

    result
}

#[tauri::command]
fn enter_input(
    app: AppHandle,
    cell_id: CellId,
    user_input: &str,
    compute_formula_channel: Channel<ComputeFormulaEvent>,
) {
    let state = app.state::<TonicState>();
    let mut spreadsheet = state
        .spreadsheet
        .lock()
        .expect("to able to lock spreadsheet in enter_input");

    // always save user input
    spreadsheet
        .user_input_raw_text
        .insert(cell_id, user_input.to_string());

    // if not entering formula, then just update value
    if !user_input.starts_with('=') {
        let insert_time = Instant::now();
        if let Ok(n) = user_input.parse::<D256>() {
            spreadsheet.sheets[0].insert(cell_id, Cell::SingleValue(CellValue::Number(n)));
            debug!(
                "User entered number ({:?}): \"{}\", to {}",
                insert_time.elapsed(),
                n,
                cell_id
            );
            let display = n.to_string();
            send_result(&compute_formula_channel, cell_id, &display);
        } else {
            spreadsheet.sheets[0].insert(
                cell_id,
                Cell::SingleValue(CellValue::Text(user_input.to_string())),
            );
            debug!(
                "User entered text ({:?}): \"{}\", to {}",
                insert_time.elapsed(),
                user_input,
                cell_id
            );
            send_result(&compute_formula_channel, cell_id, user_input);
        }
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
        send_error(&compute_formula_channel, cell_id, &msg);
        return;
    }

    // return if empty
    let Some(tokens) = lex_output.output() else {
        return;
    };

    let parse_time = Instant::now();
    let (parsed, parse_errs) = parse_formula(tokens, formula_text.len(), &mut spreadsheet);
    debug!("Parser elapsed: {:?}", parse_time.elapsed());

    // report any errors happened during parsing formula (syntax, not found name)
    if !parse_errs.is_empty() {
        let msg = parse_formula_errors_to_string(&parse_errs);
        send_error(&compute_formula_channel, cell_id, &msg);
        return;
    }

    debug!("parsed formula: {:?}", parsed);

    if let Some((exprs, _root)) = parsed {
        // report any evaluation errors
        let eval_time = Instant::now();
        let eval_result = eval_formula(cell_id, 0, exprs, &mut spreadsheet);
        debug!("Eval elapsed: {:?}", eval_time.elapsed());

        if let Err(e) = eval_result {
            let msg = format!("Eval Error: {:?}", e);
            send_error(&compute_formula_channel, cell_id, &msg);
            return;
        }

        let val = spreadsheet
            .get_cell_value(&cell_id, 0)
            .expect("to get cell value");

        debug!("evaluated formula: {:?}", val);

        let display = val.to_string();
        send_result(&compute_formula_channel, cell_id, &display);
    }
}

#[tauri::command]
fn fill_cell(app: AppHandle, source: CellId, dest: CellId) -> Option<WindowCell> {
    let state = app.state::<TonicState>();
    let mut spreadsheet = state
        .spreadsheet
        .lock()
        .expect("to able to lock spreadsheet in fill_cell");

    let source_cell = spreadsheet.sheets[0].get(&source);

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
            let raw = spreadsheet
                .user_input_raw_text
                .get(&source)
                .cloned()
                .unwrap_or_default();
            let adjusted_raw = if raw.starts_with('=') {
                format!("={}", offset_relative_refs(&raw[1..], row_off, col_off))
            } else {
                raw
            };
            let entered_text = adjusted_raw.clone();
            spreadsheet.user_input_raw_text.insert(dest, adjusted_raw);

            // eval the offset AST
            if let Err(e) = eval_formula(dest, 0, new_exprs, &mut spreadsheet) {
                return Some(WindowCell {
                    cell_id: dest,
                    display: format!("Eval Error: {:?}", e),
                    entered_text,
                });
            }

            let display = spreadsheet
                .get_cell_value(&dest, 0)
                .expect("to get cell value")
                .to_string();
            Some(WindowCell {
                cell_id: dest,
                display,
                entered_text,
            })
        }
        Some(Cell::SingleValue(value)) => {
            let value = value.clone();
            let raw = spreadsheet
                .user_input_raw_text
                .get(&source)
                .cloned()
                .unwrap_or_default();
            spreadsheet.user_input_raw_text.insert(dest, raw.clone());
            spreadsheet.sheets[0].insert(dest, Cell::SingleValue(value.clone()));
            Some(WindowCell {
                cell_id: dest,
                display: value.to_string(),
                entered_text: raw,
            })
        }
        None => None,
    }
}

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn Error + 'static>> {
    app.manage(TonicState::new());
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
            enter_input,
            fill_cell,
            get_spreadsheet_window
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
