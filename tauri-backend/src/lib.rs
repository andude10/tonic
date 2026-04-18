// How backend works
//
// Backend exposes async tauri commands to the frontend. Multiple commands can
// run concurrently. Read commands use try_read() and return empty if a mutation
// is in progress. Mutating commands are serialized via the "writing" AtomicBool flag.
//
// Frontend polls for cells that are in the current viewport (cells currently
// visible on screen) each 20ms or so (via get_cells_in_viewport command).

use std::error::Error;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::{Mutex, RwLock, RwLockWriteGuard};

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_log::log::{debug, error};

use std::collections::BTreeMap;

use crate::engine::{ChangeBounds, Engine};
use crate::parser::shift_formula_refs;
use crate::storage::grid::{CellValue, GridCellId};
use crate::storage::types::{
    AbsoluteCellId, AtomType, ExternalFunction, Projection, ProjectionFilterOption, ScriptFile,
    Sheets, Spreadsheet, Table,
};

mod call_extern_functions;
pub mod engine;
mod file_api;
mod ipc_encoding;
mod parser;
pub mod storage {
    pub mod dependency_graph;
    pub mod grid;
    pub mod name_resolution;
    pub mod stable_vec;
    pub mod types;
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

struct ViewportState {
    current_buf: Vec<u8>,
    last_viewport_buf: Vec<u8>,
    last_viewport_range: (u32, u32, u32, u32),
}

struct TonicState {
    engine: Engine,
    viewport: Mutex<ViewportState>,

    // flag: only one mutating command can run at a time.
    // checked via compare_exchange before acquiring write lock.
    writing: AtomicBool,

    file_name: Option<String>,
    file_path: Option<String>,
}

impl TonicState {
    fn new() -> Self {
        Self {
            engine: Engine::new(),
            viewport: Mutex::new(ViewportState {
                current_buf: Vec::new(),
                last_viewport_buf: Vec::new(),
                last_viewport_range: (u32::MAX, u32::MAX, u32::MAX, u32::MAX),
            }),
            writing: AtomicBool::new(false),
            file_name: None,
            file_path: None,
        }
    }
}

// shared handle to sheets, accessible without locking TonicState.
// used by resolve_reference so it can read cells during eval.
// updated when the spreadsheet is replaced (open/new file).
struct SheetsHandle(Mutex<Arc<RwLock<Sheets>>>);

impl SheetsHandle {
    fn get(&self) -> Arc<RwLock<Sheets>> {
        self.0.lock().clone()
    }
    fn update(&self, sheets: &Arc<RwLock<Sheets>>) {
        *self.0.lock() = sheets.clone();
    }
}

// RAII guard: acquires write lock and sets mutating flag.
// resets the flag on drop (even on panic).
struct MutationGuard<'a>(RwLockWriteGuard<'a, TonicState>);

impl Drop for MutationGuard<'_> {
    fn drop(&mut self) {
        self.0.writing.store(false, Ordering::Release);
    }
}

impl std::ops::Deref for MutationGuard<'_> {
    type Target = TonicState;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for MutationGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

// try to begin a mutating operation. checks the AtomicBool flag through a read lock
// (non-blocking with other readers), then acquires write lock.
// returns error if another mutation is already running.
fn begin_mutation(state: &RwLock<TonicState>) -> Result<MutationGuard<'_>, String> {
    {
        let s = state.read();
        if s.writing
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return Err("another operation in progress".to_string());
        }
    }
    Ok(MutationGuard(state.write()))
}

fn emit_save_status<R: tauri::Runtime>(app: &AppHandle<R>, state: &TonicState) {
    let _ = app.emit("save-status", state.engine.is_saved());
}

fn emit_table_projection_events<R: tauri::Runtime>(
    app: &AppHandle<R>,
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

// todo: add validation for invalid cell sizes

/// Get the editor value for a cell.
/// For regular data, returns the string representation.
/// For formulas, returns the formula string with shifted references.
/// Invalid references are rendered as error markers (for example `#REF!`).
fn get_editor_value(state: &TonicState, cell_id: CellId) -> String {
    let abs_id = cell_id.to_absolute();
    let Some(cell) = state.engine.spreadsheet.get_cell(&abs_id) else {
        return String::new();
    };

    // If defined by formula, shift references (formula_string already includes '=')
    if let Some(formula_id) = cell.defined_by_formula {
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
    cell.val.to_string()
}

#[tauri::command(async)]
fn get_editor_value_for_cell(
    state: tauri::State<'_, RwLock<TonicState>>,
    cell_id: CellId,
) -> tauri::ipc::Response {
    let Some(state) = state.try_read() else {
        return tauri::ipc::Response::new(Vec::new());
    };
    let value = get_editor_value(&state, cell_id);
    tauri::ipc::Response::new(value.into_bytes())
}

#[tauri::command(async)]
fn get_name_for_cell(
    state: tauri::State<'_, RwLock<TonicState>>,
    cell_id: CellId,
) -> tauri::ipc::Response {
    let Some(state) = state.try_read() else {
        return tauri::ipc::Response::new(Vec::new());
    };
    let abs_id = cell_id.to_absolute();
    let name = state.engine.spreadsheet.names.cell_id_to_name(&abs_id);
    tauri::ipc::Response::new(name.into_bytes())
}

#[tauri::command]
async fn rename_cell(
    state: tauri::State<'_, RwLock<TonicState>>,
    cell_id: CellId,
    name: String,
) -> Result<(), String> {
    let mut state = begin_mutation(state.inner())?;
    let abs_id = cell_id.to_absolute();
    state
        .engine
        .spreadsheet_mut()
        .names
        .create_cell_name(&name, &abs_id)
        .ok_or_else(|| format!("Name '{}' is not available", name))
}

#[tauri::command(async)]
fn get_editor_value_for_cells(
    state: tauri::State<'_, RwLock<TonicState>>,
    cells: Vec<CellId>,
) -> tauri::ipc::Response {
    let Some(state) = state.try_read() else {
        return tauri::ipc::Response::new(Vec::new());
    };
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
fn init_viewport(state: tauri::State<'_, RwLock<TonicState>>) {
    let Some(state) = state.try_read() else {
        return;
    };
    let mut vp = state.viewport.lock();
    vp.last_viewport_buf.clear();
    vp.last_viewport_range = (u32::MAX, u32::MAX, u32::MAX, u32::MAX);
}

#[tauri::command(async)]
fn is_writing(state: tauri::State<'_, RwLock<TonicState>>) -> bool {
    state.try_read().is_none()
}

#[tauri::command(async)]
fn get_cells_in_viewport(
    state: tauri::State<'_, RwLock<TonicState>>,
    sheets_handle: tauri::State<'_, SheetsHandle>,
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

    if let Some(state) = state.try_read() {
        let spreadsheet = &state.engine.spreadsheet;
        let mut vp = state.viewport.lock();

        vp.current_buf.clear();
        for row in row_start..=row_end {
            for col in col_start..=col_end {
                let id = AbsoluteCellId {
                    sheet_id: 0,
                    row,
                    col,
                };
                let cell = spreadsheet.get_projected_cell(&id);
                ipc_encoding::encode_viewport_cell(&mut vp.current_buf, row, col, cell.as_ref());
            }
        }

        let range = (row_start, row_end, col_start, col_end);
        let viewport_changed = range != vp.last_viewport_range;
        vp.last_viewport_range = range;

        if !viewport_changed && vp.current_buf == vp.last_viewport_buf {
            return tauri::ipc::Response::new(Vec::new());
        }
        let vp = &mut *vp;
        std::mem::swap(&mut vp.current_buf, &mut vp.last_viewport_buf);
        tauri::ipc::Response::new(vp.last_viewport_buf.clone())
    } else {
        // writing in progress: read cells directly from grid, show pending status
        let sheets_arc = sheets_handle.get();
        let Some(sheets) = sheets_arc.try_read() else {
            return tauri::ipc::Response::new(Vec::new());
        };
        let grid = &sheets[0];

        let mut buf = Vec::new();
        let mut has_pending = false;
        for row in row_start..=row_end {
            for col in col_start..=col_end {
                let grid_id = GridCellId { row, col };
                match grid.try_get_cell(&grid_id) {
                    None => return tauri::ipc::Response::new(Vec::new()),
                    Some(None) => ipc_encoding::encode_viewport_cell(&mut buf, row, col, None),
                    Some(Some(cell)) => {
                        let pending = cell.pending_dependencies.load(Ordering::Relaxed);
                        if pending > 0 {
                            has_pending = true;
                            buf.extend_from_slice(&row.to_le_bytes());
                            buf.extend_from_slice(&col.to_le_bytes());
                            buf.push(2); // pending flag
                            let display = pending.to_string();
                            let bytes = display.as_bytes();
                            buf.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
                            buf.extend_from_slice(bytes);
                        } else if matches!(&cell.val, CellValue::Error(s, _) if s.is_empty()) {
                            // placeholder error from mutation phase, encode as empty
                            ipc_encoding::encode_viewport_cell(&mut buf, row, col, None);
                        } else {
                            ipc_encoding::encode_viewport_cell(&mut buf, row, col, Some(&cell));
                        }
                    }
                }
            }
        }
        // no cells pending: return empty so frontend keeps its previous state
        if !has_pending {
            return tauri::ipc::Response::new(Vec::new());
        }
        tauri::ipc::Response::new(buf)
    }
}

#[tauri::command]
async fn enter_input<R: tauri::Runtime>(
    app: AppHandle<R>,
    cell_id: CellId,
    user_input: &str,
    state: tauri::State<'_, RwLock<TonicState>>,
) -> Result<(), String> {
    let mut state = begin_mutation(state.inner())?;
    let t = state.engine.start_batch();
    state
        .engine
        .parse_and_insert_string(&t, cell_id.to_absolute(), user_input);
    state.engine.end_batch(t);
    emit_save_status(&app, &state);
    Ok(())
}

#[tauri::command]
async fn delete_cells<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, RwLock<TonicState>>,
    cells: Vec<CellId>,
) -> Result<(), String> {
    let timer = std::time::Instant::now();
    let mut state = begin_mutation(state.inner())?;
    let t = state.engine.start_batch();
    for cell_id in cells {
        state.engine.delete(&t, cell_id.to_absolute());
    }
    state.engine.end_batch(t);
    debug!("Delete took: {:?}", timer.elapsed());
    emit_save_status(&app, &state);
    Ok(())
}

#[tauri::command]
async fn fill_cells<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, RwLock<TonicState>>,
    sources: Vec<CellId>,
    dests: Vec<CellId>,
    orig_min_row: u32,
    orig_max_row: u32,
    orig_min_col: u32,
    orig_max_col: u32,
) -> Result<(), String> {
    if sources.len() != dests.len() {
        return Ok(());
    }

    let timer = std::time::Instant::now();
    let mut state = begin_mutation(state.inner())?;
    let line_value = |first_value: Decimal, step: Decimal, offset: i64| {
        first_value + step * Decimal::from(offset)
    };
    let pattern_value =
        |patterns: &[Option<(Decimal, Decimal)>], pattern_index: usize, axis_offset: i64| {
            let (first_value, step) = patterns.get(pattern_index).copied().flatten()?;
            Some(line_value(first_value, step, axis_offset))
        };
    let step_value = |patterns: &[Option<(Decimal, Decimal)>], axis_offset: i64| {
        let Some((_, first_step)) = patterns.first().copied().flatten() else {
            return None;
        };

        if axis_offset >= 0 && (axis_offset as usize) < patterns.len() {
            return patterns[axis_offset as usize].map(|(_, step)| step);
        }

        if patterns.len() == 1 {
            return Some(first_step);
        }

        let (_, second_step) = patterns.get(1).copied().flatten()?;
        Some(line_value(
            first_step,
            second_step - first_step,
            axis_offset,
        ))
    };

    let get_num = |state: &TonicState, row: u32, col: u32| -> Option<Decimal> {
        let id = AbsoluteCellId {
            sheet_id: 0,
            row,
            col,
        };
        let cell = state.engine.spreadsheet.get_cell(&id)?;
        if cell.defined_by_formula.is_some() {
            return None;
        }
        match &cell.val {
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
        let source_content = state.engine.spreadsheet.get_cell(&source.to_absolute());

        if let Some(cell) = &source_content {
            if let Some(formula_id) = cell.defined_by_formula {
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
        let row_from_origin = dest.row as i64 - orig_min_row as i64;
        let col_from_origin = dest.col as i64 - orig_min_col as i64;
        let horizontal_value = pattern_value(&row_col_steps, row_idx, col_from_origin);
        let vertical_value = pattern_value(&col_row_steps, col_idx, row_from_origin);

        let extrapolated = if row_offset != 0 && col_offset != 0 {
            if let (Some(value), Some(vertical_step)) = (
                horizontal_value,
                step_value(&col_row_steps, col_from_origin),
            ) {
                Some(value + vertical_step * Decimal::from(row_offset))
            } else if let (Some(value), Some(horizontal_step)) =
                (vertical_value, step_value(&row_col_steps, row_from_origin))
            {
                Some(value + horizontal_step * Decimal::from(col_offset))
            } else {
                None
            }
        } else if row_offset != 0 {
            vertical_value
        } else if col_offset != 0 {
            horizontal_value
        } else {
            None
        };

        if let Some(val) = extrapolated {
            state.engine.insert_number(&t, dest.to_absolute(), val);
        } else if let Some(cell) = source_content {
            let text = cell.val.to_string();
            state
                .engine
                .parse_and_insert_string(&t, dest.to_absolute(), &text);
        }
    }
    state.engine.end_batch(t);
    debug!("fill_cells took: {:?}", timer.elapsed());
    emit_save_status(&app, &state);
    Ok(())
}

#[tauri::command]
async fn paste_values<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, RwLock<TonicState>>,
    cells: Vec<(CellId, String)>,
) -> Result<(), String> {
    let timer = std::time::Instant::now();
    let mut state = begin_mutation(state.inner())?;
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
    Ok(())
}

#[tauri::command]
async fn undo_input<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, RwLock<TonicState>>,
) -> Result<Option<ChangeBounds>, String> {
    let timer = std::time::Instant::now();
    let mut state = begin_mutation(state.inner())?;
    let bounds = state.engine.undo();
    debug!("Undo took: {:?}", timer.elapsed());
    emit_save_status(&app, &state);
    Ok(bounds)
}

#[tauri::command]
async fn redo_input<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, RwLock<TonicState>>,
) -> Result<Option<ChangeBounds>, String> {
    let timer = std::time::Instant::now();
    let mut state = begin_mutation(state.inner())?;
    let bounds = state.engine.redo();
    debug!("Redo took: {:?}", timer.elapsed());
    emit_save_status(&app, &state);
    Ok(bounds)
}

fn disable_table_projection_by_id(sp: &mut Spreadsheet, table_id: u32) -> Result<(), String> {
    let table = sp.tables.get(table_id).ok_or("Table not found")?;
    let proj = sp
        .projections
        .get_mut(table.projection_id)
        .ok_or("Projection not found")?;
    let num_cols = (table.body_end.col - table.body_start.col + 1) as usize;

    proj.active = false;
    proj.projected_rows = (table.body_start.row..=table.body_end.row).collect();
    proj.hidden_rows_count = 0;
    proj.filter_show_blanks = vec![true; num_cols];
    proj.filter_options_per_column
        .resize_with(num_cols, BTreeMap::new);

    // reset all filter options to selected
    for col_opts in proj.filter_options_per_column.iter_mut() {
        for opt in col_opts.values_mut() {
            opt.selected = true;
        }
    }

    Ok(())
}

fn disable_all_table_projections<R: tauri::Runtime>(
    app: &AppHandle<R>,
    sp: &mut Spreadsheet,
) -> Result<(), String> {
    let table_ids: Vec<u32> = sp
        .tables
        .iter()
        .enumerate()
        .filter_map(|(table_id, table)| table.as_ref().map(|_| table_id as u32))
        .collect();

    for table_id in table_ids {
        disable_table_projection_by_id(sp, table_id)?;
        let _ = app.emit("disable-table-projection", table_id);
        let _ = app.emit("update-table-hidden-rows", (table_id, 0u32));
    }

    Ok(())
}

#[tauri::command]
async fn insert_column<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, RwLock<TonicState>>,
    col: u32,
    left: bool,
) -> Result<(), String> {
    let timer = std::time::Instant::now();
    let mut state = begin_mutation(state.inner())?;

    state.engine.insert_column_or_row(0, false, col, left);
    disable_all_table_projections(&app, state.engine.spreadsheet_mut())?;

    debug!("insert_column took: {:?}", timer.elapsed());
    emit_save_status(&app, &state);
    Ok(())
}

#[tauri::command]
async fn insert_row<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, RwLock<TonicState>>,
    row: u32,
    below: bool,
) -> Result<(), String> {
    let timer = std::time::Instant::now();
    let mut state = begin_mutation(state.inner())?;

    state.engine.insert_column_or_row(0, true, row, !below);
    disable_all_table_projections(&app, state.engine.spreadsheet_mut())?;

    debug!("insert_row took: {:?}", timer.elapsed());
    emit_save_status(&app, &state);
    Ok(())
}

#[tauri::command]
async fn remove_column<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, RwLock<TonicState>>,
    col: u32,
) -> Result<(), String> {
    let timer = std::time::Instant::now();
    let mut state = begin_mutation(state.inner())?;

    state.engine.remove_column_or_row(0, false, col);
    disable_all_table_projections(&app, state.engine.spreadsheet_mut())?;

    debug!("remove_column took: {:?}", timer.elapsed());
    emit_save_status(&app, &state);
    Ok(())
}

#[tauri::command]
async fn remove_row<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, RwLock<TonicState>>,
    row: u32,
) -> Result<(), String> {
    let timer = std::time::Instant::now();
    let mut state = begin_mutation(state.inner())?;

    state.engine.remove_column_or_row(0, true, row);
    disable_all_table_projections(&app, state.engine.spreadsheet_mut())?;

    debug!("remove_row took: {:?}", timer.elapsed());
    emit_save_status(&app, &state);
    Ok(())
}

fn setup<R: tauri::Runtime>(app: &mut tauri::App<R>) -> Result<(), Box<dyn Error + 'static>> {
    let tonic_state = TonicState::new();
    let sheets_handle = SheetsHandle(Mutex::new(tonic_state.engine.spreadsheet.sheets.clone()));
    app.manage(RwLock::new(tonic_state));
    app.manage(sheets_handle);
    Ok(())
}

fn update_file_info(state: &mut TonicState, path: &str) {
    let p = std::path::Path::new(path);
    state.file_name = p.file_name().map(|n| n.to_string_lossy().into_owned());
    state.file_path = Some(path.to_string());
}

#[tauri::command]
async fn save_file<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, RwLock<TonicState>>,
    path: &str,
    ui_decorations_json: &str,
) -> Result<(), String> {
    let path = if path.ends_with(".tcs") {
        path.to_string()
    } else {
        format!("{}.tcs", path)
    };
    let mut state = begin_mutation(state.inner())?;
    if let Err(e) = state.engine.save_spreadsheet(&path, ui_decorations_json) {
        error!("Failed to save file '{}': {}", path, e);
        return Err(e.to_string());
    }
    update_file_info(&mut state, &path);
    emit_save_status(&app, &state);
    Ok(())
}

#[tauri::command]
async fn rename_current_file<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, RwLock<TonicState>>,
    new_name: &str,
) -> Result<(), String> {
    let mut state = begin_mutation(state.inner())?;
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

#[tauri::command]
async fn open_file<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, RwLock<TonicState>>,
    sheets_handle: tauri::State<'_, SheetsHandle>,
    path: &str,
) -> Result<String, String> {
    let mut state = begin_mutation(state.inner())?;
    let decorations = state.engine.open_spreadsheet(path).map_err(|e| {
        error!("Failed to open file '{}': {}", path, e);
        e.to_string()
    })?;
    sheets_handle.update(&state.engine.spreadsheet.sheets);
    {
        let mut vp = state.viewport.lock();
        vp.last_viewport_buf.clear();
        vp.last_viewport_range = (u32::MAX, u32::MAX, u32::MAX, u32::MAX);
    }
    update_file_info(&mut state, path);
    emit_save_status(&app, &state);
    Ok(decorations)
}

#[tauri::command]
async fn new_file<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, RwLock<TonicState>>,
    sheets_handle: tauri::State<'_, SheetsHandle>,
) -> Result<(), String> {
    let mut state = begin_mutation(state.inner())?;
    state.file_name = Some("Untitled.tcv".to_string());
    state.file_path = None;
    state.engine.create_empty_spreadsheet();
    sheets_handle.update(&state.engine.spreadsheet.sheets);
    emit_save_status(&app, &state);
    Ok(())
}

#[tauri::command(async)]
fn get_file_info(state: tauri::State<'_, RwLock<TonicState>>) -> (Option<String>, Option<String>) {
    let Some(state) = state.try_read() else {
        return (None, None);
    };
    (state.file_name.clone(), state.file_path.clone())
}

#[tauri::command(async)]
fn get_dependency_graph_dot(state: tauri::State<'_, RwLock<TonicState>>) -> String {
    let Some(state) = state.try_read() else {
        return String::new();
    };
    state.engine.spreadsheet.dependency_graph.to_dot()
}

// -- extension / script management commands --

#[tauri::command]
async fn register_function(
    state: tauri::State<'_, RwLock<TonicState>>,
    name: String,
    args: Vec<String>,
    file_name: String,
) -> Result<(), String> {
    let mut state = begin_mutation(state.inner())?;
    let arg_types: Vec<AtomType> = args
        .iter()
        .map(|s| match s.as_str() {
            "number" | "any" => Ok(AtomType::Number),
            "text" => Ok(AtomType::Text),
            "boolean" => Ok(AtomType::Bool),
            "reference" => Ok(AtomType::Reference),
            other => Err(format!("unknown param type '{}'", other)),
        })
        .collect::<Result<_, _>>()?;

    let sp = state.engine.spreadsheet_mut();
    if sp.names.user_function_names.contains_key(&name) {
        return Err(format!("function '{}' already registered", name));
    }

    let func_id = sp.external_functions.insert(ExternalFunction {
        name: name.clone(),
        args: arg_types,
        file_name,
    });
    sp.names.user_function_names.insert(name.clone(), func_id);
    sp.names.user_function_names_lookup.insert(func_id, name);
    Ok(())
}

#[tauri::command]
async fn unregister_functions_by_file(
    state: tauri::State<'_, RwLock<TonicState>>,
    file_name: String,
) -> Result<(), String> {
    let mut state = begin_mutation(state.inner())?;
    let sp = state.engine.spreadsheet_mut();

    let to_remove: Vec<(u32, String)> = sp
        .external_functions
        .iter()
        .enumerate()
        .filter_map(|(id, f)| {
            f.as_ref()
                .filter(|f| f.file_name == file_name)
                .map(|f| (id as u32, f.name.clone()))
        })
        .collect();

    for (func_id, name) in to_remove {
        sp.external_functions[func_id as usize] = None;
        sp.names.user_function_names.remove(&name);
        sp.names.user_function_names_lookup.remove(&func_id);
    }
    Ok(())
}

#[tauri::command]
async fn add_script(
    state: tauri::State<'_, RwLock<TonicState>>,
    file_path: String,
) -> Result<String, String> {
    let content = std::fs::read_to_string(&file_path).map_err(|e| e.to_string())?;
    let name = std::path::Path::new(&file_path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "script.js".into());

    let mut state = begin_mutation(state.inner())?;
    let sp = state.engine.spreadsheet_mut();
    if sp.scripts.iter().any(|s| s.name == name) {
        return Err(format!("script '{}' already exists", name));
    }
    sp.scripts.push(ScriptFile {
        name,
        content: content.clone(),
    });
    Ok(content)
}

#[tauri::command]
async fn remove_script(
    state: tauri::State<'_, RwLock<TonicState>>,
    name: String,
) -> Result<(), String> {
    let mut state = begin_mutation(state.inner())?;
    let sp = state.engine.spreadsheet_mut();
    sp.scripts.retain(|s| s.name != name);
    Ok(())
}

#[tauri::command(async)]
fn list_scripts(state: tauri::State<'_, RwLock<TonicState>>) -> Vec<String> {
    let Some(state) = state.try_read() else {
        return Vec::new();
    };
    state
        .engine
        .spreadsheet
        .scripts
        .iter()
        .map(|s| s.name.clone())
        .collect()
}

#[tauri::command(async)]
fn get_script_content(state: tauri::State<'_, RwLock<TonicState>>, name: String) -> Option<String> {
    let state = state.try_read()?;
    state
        .engine
        .spreadsheet
        .scripts
        .iter()
        .find(|s| s.name == name)
        .map(|s| s.content.clone())
}

#[tauri::command]
async fn create_table(
    state: tauri::State<'_, RwLock<TonicState>>,
    table_name: String,
    first_header: GridCellId,
    last_header: GridCellId,
    body_start: GridCellId,
    body_end: GridCellId,
) -> Result<u32, String> {
    let mut state = begin_mutation(state.inner())?;
    let sp = state.engine.spreadsheet_mut();

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
                if let Some(val) = sp.sheets.read()[0].get_value(&GridCellId { row, col }) {
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

#[tauri::command]
async fn change_table_name(
    state: tauri::State<'_, RwLock<TonicState>>,
    old_name: String,
    new_name: String,
) -> Result<(), String> {
    let mut state = begin_mutation(state.inner())?;
    let sp = state.engine.spreadsheet_mut();

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

#[tauri::command]
async fn toggle_table_sort<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, RwLock<TonicState>>,
    header: GridCellId,
    desc: bool,
) -> Result<(), String> {
    let timer = std::time::Instant::now();

    // todo: remove code duplication with other sort and filter commands

    let mut state = begin_mutation(state.inner())?;
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

#[tauri::command]
async fn toggle_table_filter<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, RwLock<TonicState>>,
    header: GridCellId,
    filter_option_id: u32,
) -> Result<(), String> {
    let timer = std::time::Instant::now();

    let mut state = begin_mutation(state.inner())?;
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

#[tauri::command]
async fn select_all_table_filters<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, RwLock<TonicState>>,
    header: GridCellId,
) -> Result<(), String> {
    let timer = std::time::Instant::now();

    let mut state = begin_mutation(state.inner())?;
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

#[tauri::command]
async fn clear_all_table_filters<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, RwLock<TonicState>>,
    header: GridCellId,
) -> Result<(), String> {
    let timer = std::time::Instant::now();

    let mut state = begin_mutation(state.inner())?;
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

#[tauri::command]
async fn apply_table_projection<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, RwLock<TonicState>>,
    table_name: String,
) -> Result<(), String> {
    let timer = std::time::Instant::now();
    let mut state = begin_mutation(state.inner())?;
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
                .map(|&row| sp.sheets.read()[sheet_id as usize].get_value(&GridCellId { row, col }))
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
        .spreadsheet_mut()
        .projections
        .get_mut(proj_id)
        .ok_or("Projection not found")?;
    let num_cols = (col_end - col_start + 1) as usize;
    proj.active = false;
    proj.projected_rows = (body_start_row..=body_end_row).collect();
    proj.hidden_rows_count = 0;
    proj.filter_show_blanks = vec![true; num_cols];

    state.engine.end_batch(t);
    debug!("apply_table_projection took: {:?}", timer.elapsed());

    let _ = app.emit("disable-table-projection", table_id);
    emit_save_status(&app, &state);
    Ok(())
}

#[tauri::command]
async fn disable_table_projection<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, RwLock<TonicState>>,
    table_name: String,
) -> Result<(), String> {
    let mut state = begin_mutation(state.inner())?;
    let sp = state.engine.spreadsheet_mut();

    let &table_id = sp
        .names
        .table_names
        .get(&table_name)
        .ok_or_else(|| format!("Table '{}' not found", table_name))?;
    disable_table_projection_by_id(sp, table_id)?;

    let _ = app.emit("disable-table-projection", table_id);
    let _ = app.emit("update-table-hidden-rows", (table_id, 0u32));
    Ok(())
}

#[derive(Serialize)]
struct FilterOptionResponse {
    id: u32,
    val: String,
    selected: bool,
}

#[tauri::command]
async fn get_filter_options_for_table_column(
    state: tauri::State<'_, RwLock<TonicState>>,
    header: GridCellId,
) -> Result<Vec<FilterOptionResponse>, String> {
    let timer = std::time::Instant::now();
    let Some(state) = state.try_read() else {
        return Ok(Vec::new());
    };
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

fn build_app_inner<R: tauri::Runtime>(
    builder: tauri::Builder<R>,
    enable_log_plugin: bool,
) -> tauri::Builder<R> {
    let builder = if enable_log_plugin {
        builder.plugin(
            tauri_plugin_log::Builder::new()
                .target(tauri_plugin_log::Target::new(
                    tauri_plugin_log::TargetKind::Webview,
                ))
                .build(),
        )
    } else {
        builder
    };

    builder
        .plugin(tauri_plugin_dialog::init())
        .setup(setup)
        .invoke_handler(tauri::generate_handler![
            is_writing,
            init_viewport,
            enter_input,
            fill_cells,
            delete_cells,
            paste_values,
            insert_column,
            insert_row,
            remove_column,
            remove_row,
            get_cells_in_viewport,
            get_editor_value_for_cell,
            get_editor_value_for_cells,
            get_name_for_cell,
            rename_cell,
            save_file,
            open_file,
            new_file,
            get_file_info,
            get_dependency_graph_dot,
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
            crate::call_extern_functions::ext_fn_poll,
            register_function,
            unregister_functions_by_file,
            add_script,
            remove_script,
            list_scripts,
            get_script_content,
        ])
}

fn build_app<R: tauri::Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    build_app_inner(builder, true)
}

#[cfg(test)]
fn build_test_app<R: tauri::Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    build_app_inner(builder, false)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    build_app(tauri::Builder::default())
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests;
