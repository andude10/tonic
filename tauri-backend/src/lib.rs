use std::{error::Error, sync::Mutex, time::Instant};

use ::serde::{Deserialize, Serialize};
use fastnum::D256;
use tauri::{ipc::Channel, AppHandle, Emitter, Listener, Manager};
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
    spreadsheet_viewport: (CellId, CellId),
}

impl TonicState {
    fn new() -> Self {
        Self {
            spreadsheet: Spreadsheet::new(),
            spreadsheet_viewport: (CellId { col: 0, row: 0 }, CellId { col: 0, row: 0 }),
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

/// Send a render-cell event for a single cell, but only if the cell is within
/// the current render window. Expects state to be already locked.
fn send_display_cell(app: &AppHandle, state: &TonicState, cell_id: CellId) {
    let (start, end) = state.spreadsheet_viewport;
    if cell_id.row < start.row
        || cell_id.row > end.row
        || cell_id.col < start.col
        || cell_id.col > end.col
    {
        return;
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

    let _ = app.emit(
        "display-cells",
        vec![DisplayCellEvent {
            cell_id,
            display,
            entered_text,
            is_error,
        }],
    );
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
        send_display_cell(&app, &state, cell_id);
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
        send_display_cell(&app, &state, cell_id);
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
        send_display_cell(&app, &state, cell_id);
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

        send_display_cell(&app, &state, cell_id);
    }
}

#[tauri::command]
fn fill_cell(app: AppHandle, source: CellId, dest: CellId, before_source: Option<CellId>) {
    let state = app.state::<Mutex<TonicState>>();
    let mut state = state
        .lock()
        .expect("to able to lock spreadsheet in fill_cell");

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
            send_display_cell(&app, &state, dest);
            return;
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
            send_display_cell(&app, &state, dest);
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
            send_display_cell(&app, &state, dest);
        }
        Some(Cell::FormulaError { .. }) | None => {}
    }
}

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn Error + 'static>> {
    app.manage(Mutex::new(TonicState::new()));

    let handle = app.handle().clone();
    app.listen("spreadsheet-viewport-changed", move |event| {
        #[derive(Deserialize, Debug)]
        #[serde(rename_all = "camelCase")]
        struct Payload {
            start: CellId,
            end: CellId,
            #[serde(default)]
            full_refresh: bool,
        }

        let Ok(payload) = serde_json::from_str::<Payload>(event.payload()) else {
            return;
        };

        let state = handle.state::<Mutex<TonicState>>();
        let mut state = state.lock().unwrap();

        // save new spreadsheet_viewport, get old one for diff
        let (old_start, old_end) = state.spreadsheet_viewport;
        state.spreadsheet_viewport = (payload.start, payload.end);

        // Compute difference rectangles: regions in the new viewport but not the old.
        // Up to 4 strips: top, bottom, left, right (non-overlapping).
        let new_s = payload.start;
        let new_e = payload.end;
        let mut diff_rects: Vec<(CellId, CellId)> = Vec::new();

        // Check if old viewport is uninitialized (both zero), full refresh, or no overlap
        let has_overlap = !payload.full_refresh
            && (old_start != old_end || (old_start.row != 0 || old_start.col != 0));
        let overlaps = has_overlap
            && new_s.row <= old_end.row
            && new_e.row >= old_start.row
            && new_s.col <= old_end.col
            && new_e.col >= old_start.col;

        if !overlaps {
            // No overlap: entire new viewport is the diff
            diff_rects.push((new_s, new_e));
        } else {
            // Top strip: new rows above the old viewport
            if new_s.row < old_start.row {
                diff_rects.push((
                    CellId {
                        col: new_s.col,
                        row: new_s.row,
                    },
                    CellId {
                        col: new_e.col,
                        row: old_start.row - 1,
                    },
                ));
            }
            // Bottom strip: new rows below the old viewport
            if new_e.row > old_end.row {
                diff_rects.push((
                    CellId {
                        col: new_s.col,
                        row: old_end.row + 1,
                    },
                    CellId {
                        col: new_e.col,
                        row: new_e.row,
                    },
                ));
            }
            // Left strip: new cols left of old, only for overlapping rows
            let row_lo = new_s.row.max(old_start.row);
            let row_hi = new_e.row.min(old_end.row);
            if new_s.col < old_start.col && row_lo <= row_hi {
                diff_rects.push((
                    CellId {
                        col: new_s.col,
                        row: row_lo,
                    },
                    CellId {
                        col: old_start.col - 1,
                        row: row_hi,
                    },
                ));
            }
            // Right strip: new cols right of old, only for overlapping rows
            if new_e.col > old_end.col && row_lo <= row_hi {
                diff_rects.push((
                    CellId {
                        col: old_end.col + 1,
                        row: row_lo,
                    },
                    CellId {
                        col: new_e.col,
                        row: row_hi,
                    },
                ));
            }
        }

        // Collect cells from each difference rectangle
        let sheet = &state.spreadsheet.sheets[0];
        let mut batch = Vec::new();
        for (rect_start, rect_end) in &diff_rects {
            for col in rect_start.col..=rect_end.col {
                let lo = CellId {
                    col,
                    row: rect_start.row,
                };
                let hi = CellId {
                    col,
                    row: rect_end.row,
                };
                for (&cell_id, cell) in sheet.range(lo..=hi) {
                    let (display, is_error) = match cell {
                        Cell::SingleValue(v) => (v.to_string(), false),
                        Cell::Formula { value, .. } => (value.to_string(), false),
                        Cell::FormulaError { error } => (error.clone(), true),
                    };
                    let entered_text = state
                        .spreadsheet
                        .user_input_raw_text
                        .get(&cell_id)
                        .cloned()
                        .unwrap_or_default();

                    batch.push(DisplayCellEvent {
                        cell_id,
                        display,
                        entered_text,
                        is_error,
                    });
                }
            }
        }
        if !batch.is_empty() {
            let _ = handle.emit("display-cells", batch);
        }
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
        .invoke_handler(tauri::generate_handler![enter_input, fill_cell])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
