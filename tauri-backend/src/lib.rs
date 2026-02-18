use std::{error::Error, sync::Mutex};

use ::serde::Serialize;
use chumsky::{prelude::SimpleSpan, span::Span, Parser};
use tauri::{ipc::Channel, AppHandle, Manager};
use tauri_plugin_log::log::debug;

use crate::{
    parser::{create_lexer, parse_errors_to_string, parse_formula, token_errors_to_string},
    sheet::{CellId, CellValue, Spreadsheet},
};

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
    compute_fromula_channel: Channel<ComputeFormulaEvent>,
) {
    let state = app.state::<TonicState>();
    let mut spreadsheet = state
        .spreadsheet
        .lock()
        .expect("to able to lock spreadsheet in enter_input");

    // if not entering formula, then just update value
    if !user_input.starts_with('=') {
        let value = match user_input.parse::<f64>() {
            Ok(n) => CellValue::Number(n),
            Err(_) => CellValue::Text(user_input.to_string()),
        };
        spreadsheet.sheets[0].insert(cell_id, value);
        return;
    }

    // parse formula
    let formula_text = &user_input[1..];

    let lex_output = create_lexer().parse(formula_text);
    if lex_output.has_errors() {
        let msg = parse_errors_to_string(lex_output.errors());
        let _ = compute_fromula_channel.send(ComputeFormulaEvent::ParseErr {
            cell_id,
            message: &msg,
        });
        return;
    }

    let Some(tokens) = lex_output.output() else {
        return;
    };

    let eoi = SimpleSpan::new((), formula_text.len()..formula_text.len());
    let (parsed, parse_errs) = parse_formula(tokens, eoi);

    if !parse_errs.is_empty() {
        let msg = token_errors_to_string(&parse_errs);
        let _ = compute_fromula_channel.send(ComputeFormulaEvent::ParseErr {
            cell_id,
            message: &msg,
        });
        return;
    }

    if let Some((exprs, root)) = parsed {
        debug!("formula parsed: root={}, exprs={:?}", root, exprs);
        spreadsheet.sheets[0].insert(cell_id, CellValue::Formula(exprs));
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
                    tauri_plugin_log::TargetKind::Stdout,
                ))
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
