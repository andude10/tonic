use std::{error::Error, sync::Mutex, time::Instant};

use ::serde::Serialize;
use chumsky::{span::Span, Parser};
use tauri::{ipc::Channel, AppHandle, Manager};
use tauri_plugin_log::log::{debug, error};

use crate::{
    engine::eval_formula,
    parser::{lex_formula, lexer_errors_to_string, parse_formula, parse_formula_errors_to_string},
    sheet::{Cell, CellId, CellValue, Spreadsheet},
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

// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
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

    // if not entering formula, then just update value
    if !user_input.starts_with('=') {
        if let Ok(n) = user_input.parse::<f64>() {
            spreadsheet.sheets[0].insert(cell_id, Cell::SingleValue(CellValue::Number(n)));
            debug!("User entered number: \"{}\", to {}", n, cell_id);
            let display = n.to_string();
            send_result(&compute_formula_channel, cell_id, &display);
        } else {
            spreadsheet.sheets[0].insert(
                cell_id,
                Cell::SingleValue(CellValue::Text(user_input.to_string())),
            );
            debug!("User entered text: \"{}\", to {}", user_input, cell_id);
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
        .invoke_handler(tauri::generate_handler![greet, enter_input])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
