use std::{error::Error, sync::Mutex, time::Instant};

use ::serde::{Deserialize, Serialize};
use fastnum::D256;
use tauri::{AppHandle, Emitter, Listener, Manager};
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

#[derive(Deserialize, Debug, Copy, Clone)]
#[serde(rename_all = "camelCase")]
struct SpreadsheetViewport {
    row_start: u32,
    row_end: u32,
}

struct TonicState {
    spreadsheet: Spreadsheet,
    spreadsheet_viewport: SpreadsheetViewport,
}

impl TonicState {
    fn new() -> Self {
        Self {
            spreadsheet: Spreadsheet::new(),
            spreadsheet_viewport: SpreadsheetViewport {
                row_start: 0,
                row_end: 0,
            },
        }
    }
}

// todo: rename "RenderCellEvent", "render-window-changed", etc.
// "RenderWindow" means part of the spreadsheet which is displayed by frontend
// "SpreadsheetViewport" might be a better name

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DisplayCellEvent {
    cell_id: CellId,
    display: String,
    entered_text: String,
    is_error: bool,
}

/// Build display data for a single cell, but only if the cell is within
/// the current render window. Expects state to be already locked.
fn build_display_cell(state: &TonicState, cell_id: CellId) -> Option<DisplayCellEvent> {
    let vp = &state.spreadsheet_viewport;
    if cell_id.row < vp.row_start || cell_id.row > vp.row_end {
        return None;
    }

    let (display, is_error) = match state.spreadsheet.sheets[0].get(&cell_id) {
        Some(Cell::SingleValue(v)) => (v.to_string(), false),
        Some(Cell::Formula { value, .. }) => (value.to_string(), false),
        Some(Cell::FormulaError { error }) => (error.clone(), true),
        None => (String::new(), false),
    };
    let entered_text = state
        .spreadsheet
        .user_input_raw_text
        .get(&cell_id)
        .cloned()
        .unwrap_or_default();

    Some(DisplayCellEvent {
        cell_id,
        display,
        entered_text,
        is_error,
    })
}

/// Emit a batch of display-cell updates in a single IPC event.
fn emit_display_cells(app: &AppHandle, cells: Vec<DisplayCellEvent>) {
    if cells.is_empty() {
        return;
    }
    app.emit("display-cells", cells)
        .expect("to be able to emit display-cells");
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
        emit_display_cells(
            &app,
            build_display_cell(&state, cell_id).into_iter().collect(),
        );
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
        emit_display_cells(
            &app,
            build_display_cell(&state, cell_id).into_iter().collect(),
        );
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
        emit_display_cells(
            &app,
            build_display_cell(&state, cell_id).into_iter().collect(),
        );
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
        debug!("Eval elapsed: {:?}", eval_time.elapsed());

        emit_display_cells(
            &app,
            build_display_cell(&state, cell_id).into_iter().collect(),
        );
    }
}

#[tauri::command]
fn delete_cells(app: AppHandle, cells: Vec<CellId>) {
    let state = app.state::<Mutex<TonicState>>();
    let mut state = state
        .lock()
        .expect("to able to lock spreadsheet in delete_cells");

    for cell_id in &cells {
        state
            .spreadsheet
            .user_input_raw_text
            .insert(*cell_id, String::new());
        state.spreadsheet.sheets[0]
            .insert(*cell_id, Cell::SingleValue(CellValue::Text(String::new())));
    }
    let updates: Vec<_> = cells
        .iter()
        .filter_map(|&id| build_display_cell(&state, id))
        .collect();
    emit_display_cells(&app, updates);
}

#[tauri::command]
fn fill_cells(
    app: AppHandle,
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

    let state = app.state::<Mutex<TonicState>>();
    let mut state = state
        .lock()
        .expect("to able to lock spreadsheet in fill_cells");

    let mut updated_cells = Vec::with_capacity(dests.len());

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
                updated_cells.push(dest);
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
                updated_cells.push(dest);
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
                updated_cells.push(dest);
            }
            Some(Cell::FormulaError { .. }) | None => {}
        }
    }

    let updates: Vec<_> = updated_cells
        .iter()
        .filter_map(|&id| build_display_cell(&state, id))
        .collect();
    emit_display_cells(&app, updates);
}

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn Error + 'static>> {
    app.manage(Mutex::new(TonicState::new()));

    let handle = app.handle().clone();
    app.listen("spreadsheet-viewport-changed", move |event| {
        let Ok(viewport) = serde_json::from_str::<SpreadsheetViewport>(event.payload()) else {
            return;
        };

        let state = handle.state::<Mutex<TonicState>>();
        let mut state = state.lock().unwrap();

        state.spreadsheet_viewport = viewport;

        let sheet = &state.spreadsheet.sheets[0];
        let lo = CellId {
            col: 0,
            row: viewport.row_start,
        };
        let hi = CellId {
            col: u32::MAX,
            row: viewport.row_end,
        };
        let updates: Vec<_> = sheet
            .range(lo..=hi)
            .filter_map(|(&cell_id, _)| build_display_cell(&state, cell_id))
            .collect();
        emit_display_cells(&handle, updates);
    });

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
            fill_cells,
            delete_cells
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
