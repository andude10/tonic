use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;
use std::sync::atomic::{self, AtomicU32, AtomicU64};
use std::sync::Arc;
use std::time::Instant;

use parking_lot::RwLock;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Listener};
use tauri_plugin_log::log::{debug, info};

use std::io;

use crate::file_api;
use crate::parser::{
    format_eval_error, format_lex_error, format_parse_error, lex_formula, parse_formula,
    FormulaState,
};
use crate::storage::grid::{Cell, CellValue, GridCellId};
use crate::storage::types::{
    AbsoluteCellId, AtomType, CellRange, Coordinate, Expr, ExprAtom, ExprId, Formula, FormulaId,
    ProjectionFilterOption, Reference, SheetId, Sheets, Spreadsheet,
};

/// A single cell mutation: (cell_id, old_value, new_value).
#[derive(Clone)]
pub struct CellUpdate(pub AbsoluteCellId, pub Option<Cell>, pub Option<Cell>);

// argument sent to JS for external function calls
// literal values are sent directly, references as coordinates
// (the JS dispatcher resolves refs via resolve_reference command)
#[derive(Serialize, Clone, Debug)]
#[serde(tag = "type", content = "value")]
enum ExternArg {
    #[serde(rename = "number")]
    Number(String),
    #[serde(rename = "text")]
    Text(String),
    #[serde(rename = "boolean")]
    Boolean(bool),
    #[serde(rename = "single_ref")]
    SingleRef { sheet_id: u32, row: u32, col: u32 },
    #[serde(rename = "range_ref")]
    RangeRef {
        sheet_id: u32,
        start_row: u32,
        start_col: u32,
        end_row: u32,
        end_col: u32,
    },
}

static CALL_ID: AtomicU64 = AtomicU64::new(0);

/// Dedicated thread pool for parallel formula evaluation (heartbeat scheduling).
static EVAL_POOL: forte::ThreadPool = forte::ThreadPool::new();

// --- external function IPC bridge ---
//
// flow: forte worker → EXT_CALL_TX → ipc thread → app.emit(batch) → JS
//       JS → app.emit(response per call) → persistent listener → resp_tx → forte worker

struct ExtCallRequest {
    call_id: u64,
    func_name: String,
    args: Vec<ExternArg>,
    resp_tx: std::sync::mpsc::Sender<ExtFnResponse>,
}

// response with callId so the persistent listener can route it
#[derive(Deserialize)]
struct ExtFnResponseWithId {
    #[serde(rename = "callId")]
    call_id: u64,
    #[serde(flatten)]
    resp: ExtFnResponse,
}

static EXT_CALL_TX: std::sync::OnceLock<std::sync::mpsc::Sender<ExtCallRequest>> =
    std::sync::OnceLock::new();

/// Start the IPC bridge thread. Must be called once during app setup with the AppHandle.
pub fn set_app_handle<R: tauri::Runtime>(app: AppHandle<R>) {
    let (tx, rx) = std::sync::mpsc::channel::<ExtCallRequest>();
    EXT_CALL_TX.set(tx).ok();

    // pending calls: callId → response sender. shared between the listener and the IPC thread.
    let pending: Arc<std::sync::Mutex<HashMap<u64, std::sync::mpsc::Sender<ExtFnResponse>>>> =
        Arc::new(std::sync::Mutex::new(HashMap::new()));

    // single persistent listener for all responses (no per-call app.once)
    let listener_pending = pending.clone();
    app.listen("ext-fn-response", move |event: tauri::Event| {
        if let Ok(resp) = serde_json::from_str::<ExtFnResponseWithId>(event.payload()) {
            if let Some(tx) = listener_pending.lock().unwrap().remove(&resp.call_id) {
                let _ = tx.send(resp.resp);
            }
        }
    });

    std::thread::Builder::new()
        .name("ext-ipc".into())
        .spawn(move || {
            const BATCH_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1);

            loop {
                // block until at least one request arrives
                let first = match rx.recv() {
                    Ok(req) => req,
                    Err(_) => break, // channel closed
                };

                // drain all immediately available requests into a batch
                let mut batch = vec![first];
                while let Ok(req) = rx.recv_timeout(BATCH_TIMEOUT) {
                    batch.push(req);
                }

                // register all response channels and build the batch payload
                let mut calls = Vec::with_capacity(batch.len());
                {
                    let mut map = pending.lock().unwrap();
                    for req in batch {
                        map.insert(req.call_id, req.resp_tx);
                        calls.push(serde_json::json!({
                            "callId": req.call_id,
                            "funcName": req.func_name,
                            "args": req.args,
                        }));
                    }
                }

                // single emit with all calls
                let _ = app.emit("ext-fn-call-batch", calls);
            }
        })
        .expect("failed to spawn ext-ipc thread");
}

// response from a JS extension function call
#[derive(Deserialize)]
struct ExtFnResponse {
    #[serde(default)]
    v: Option<String>,
    #[serde(default)]
    t: Option<String>,
    #[serde(default)]
    e: Option<String>,
}

// call a JS-side external function.
// sends the request through EXT_CALL_TX to the IPC thread and blocks on the response.
// forte workers never touch Tauri APIs — all IPC goes through the dedicated thread.
fn call_extern_js_function(func_name: &str, args: Vec<ExternArg>) -> Result<CellValue, EvalError> {
    let tx = EXT_CALL_TX
        .get()
        .ok_or_else(|| EvalError::Error("ext-ipc not initialized".into()))?;

    let call_id = CALL_ID.fetch_add(1, atomic::Ordering::Relaxed);
    let (resp_tx, resp_rx) = std::sync::mpsc::channel();

    tx.send(ExtCallRequest {
        call_id,
        func_name: func_name.to_string(),
        args,
        resp_tx,
    })
    .map_err(|_| EvalError::Error("ext-ipc channel closed".into()))?;

    let resp = resp_rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .map_err(|e| match e {
            std::sync::mpsc::RecvTimeoutError::Timeout => {
                EvalError::Error(format!("{}(): timeout", func_name))
            }
            std::sync::mpsc::RecvTimeoutError::Disconnected => {
                EvalError::Error(format!("{}(): ipc channel dropped", func_name))
            }
        })?;

    if let Some(err) = resp.e {
        return Err(EvalError::Error(err));
    }

    let val = resp.v.unwrap_or_default();
    match resp.t.as_deref() {
        Some("number") => match val.parse::<Decimal>() {
            Ok(n) => Ok(CellValue::Number(n)),
            Err(_) => Ok(CellValue::Text(val.into())),
        },
        Some("boolean") => Ok(CellValue::Text(val.into())),
        _ => Ok(CellValue::Text(val.into())),
    }
}

/// Bounding rectangle of affected cells. Returned by undo/redo.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ChangeBounds {
    pub min_row: u32,
    pub max_row: u32,
    pub min_col: u32,
    pub max_col: u32,
}

enum HistoryLogEntry {
    Update {
        id: u64,
        changes: Vec<CellUpdate>,
        bounds: ChangeBounds,
    },
    InsertRow {
        id: u64,
        changes: Vec<CellUpdate>,
        bounds: ChangeBounds,
    },
    InsertColumn {
        id: u64,
        changes: Vec<CellUpdate>,
        bounds: ChangeBounds,
    },
    RemoveRow {
        id: u64,
        changes: Vec<CellUpdate>,
        bounds: ChangeBounds,
    },
    RemoveColumn {
        id: u64,
        changes: Vec<CellUpdate>,
        bounds: ChangeBounds,
    },
}

impl HistoryLogEntry {
    fn id(&self) -> u64 {
        match self {
            HistoryLogEntry::Update { id, .. }
            | HistoryLogEntry::InsertRow { id, .. }
            | HistoryLogEntry::InsertColumn { id, .. }
            | HistoryLogEntry::RemoveRow { id, .. }
            | HistoryLogEntry::RemoveColumn { id, .. } => *id,
        }
    }

    fn bounds(&self) -> &ChangeBounds {
        match self {
            HistoryLogEntry::Update { bounds, .. }
            | HistoryLogEntry::InsertRow { bounds, .. }
            | HistoryLogEntry::InsertColumn { bounds, .. }
            | HistoryLogEntry::RemoveRow { bounds, .. }
            | HistoryLogEntry::RemoveColumn { bounds, .. } => bounds,
        }
    }

    fn changes(&self) -> &Vec<CellUpdate> {
        match self {
            HistoryLogEntry::Update { changes, .. }
            | HistoryLogEntry::InsertRow { changes, .. }
            | HistoryLogEntry::InsertColumn { changes, .. }
            | HistoryLogEntry::RemoveRow { changes, .. }
            | HistoryLogEntry::RemoveColumn { changes, .. } => changes,
        }
    }
}

/// Tracks undo/redo history.
pub struct History {
    log: Vec<HistoryLogEntry>,
    log_position: usize,
    next_log_id: u64,
    last_saved_log_id: Option<u64>,
}

impl History {
    fn new() -> Self {
        Self {
            log: Vec::new(),
            log_position: 0,
            next_log_id: 1,
            last_saved_log_id: None,
        }
    }
}

pub struct EngineGuard(PhantomData<()>);

#[derive(Debug)]
pub enum EvalError {
    TypeError {
        expected: AtomType,
        got: AtomType,
        span: Option<(u32, u32)>,
    },
    DivisionByZero {
        span: Option<(u32, u32)>,
    },
    Error(String),
}

impl EvalError {
    fn with_span(self, span: Option<(u32, u32)>) -> Self {
        match self {
            EvalError::TypeError { expected, got, .. } => EvalError::TypeError {
                expected,
                got,
                span,
            },
            EvalError::DivisionByZero { .. } => EvalError::DivisionByZero { span },
            other => other,
        }
    }
}

struct DebugInfo {
    eval_number: u64,
}

impl DebugInfo {
    fn new() -> Self {
        Self { eval_number: 0 }
    }
}

pub struct Engine {
    // todo: move spreadsheet out of the Engine?
    pub spreadsheet: Arc<Spreadsheet>,
    history: History,
    batch: Vec<CellUpdate>,
    debug: DebugInfo,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ColumnOrRowChange {
    Insert,
    Remove,
}

#[derive(Clone, Copy)]
struct ColumnOrRowChangeSpec {
    change: ColumnOrRowChange,
    sheet_id: u32,
    changing_row: bool,
    index: u32,
    shift_start: u32,
    deleted_index: Option<u32>,
}

impl ColumnOrRowChangeSpec {
    fn for_insert(sheet_id: u32, changing_row: bool, index: u32, include_index: bool) -> Self {
        let shift_start = if include_index {
            index
        } else {
            index.saturating_add(1)
        };
        Self {
            change: ColumnOrRowChange::Insert,
            sheet_id,
            changing_row,
            index,
            shift_start,
            deleted_index: None,
        }
    }

    fn for_remove(sheet_id: u32, changing_row: bool, index: u32) -> Self {
        Self {
            change: ColumnOrRowChange::Remove,
            sheet_id,
            changing_row,
            index,
            shift_start: index.saturating_add(1),
            deleted_index: Some(index),
        }
    }

    fn operation_name(&self) -> &'static str {
        if self.change == ColumnOrRowChange::Insert {
            "Insert"
        } else {
            "Remove"
        }
    }
}

// todo: add more comments (explain what happens with dependencies, dependants and the grid)

impl Engine {
    pub fn new() -> Self {
        EVAL_POOL.resize_to_available();

        Self {
            spreadsheet: Arc::new(Spreadsheet::new()),
            history: History::new(),
            batch: Vec::new(),
            debug: DebugInfo::new(),
        }
    }

    /// Exclusive mutable access to the spreadsheet. Only valid when no eval tasks hold a reference.
    pub fn spreadsheet_mut(&mut self) -> &mut Spreadsheet {
        Arc::get_mut(&mut self.spreadsheet).expect("spreadsheet is shared during eval")
    }

    pub fn start_batch(&mut self) -> EngineGuard {
        self.batch.clear();
        EngineGuard(PhantomData)
    }

    fn set_cell(&mut self, id: &AbsoluteCellId, new_cell: Option<Cell>) -> Option<Cell> {
        let old_cell = self.spreadsheet.get_cell(id);
        if old_cell
            .as_ref()
            .and_then(|cell| cell.defined_by_formula)
            .is_some()
        {
            self.spreadsheet_mut()
                .dependency_graph
                .remove_formula_cell(*id);
        }

        match new_cell.as_ref() {
            Some(cell) => self.spreadsheet.insert_cell(id, cell.clone()),
            None => self.spreadsheet.remove_cell(id),
        }

        if let Some(formula_id) = new_cell.as_ref().and_then(|cell| cell.defined_by_formula) {
            let ast = self
                .spreadsheet
                .formulas
                .get(formula_id)
                .map(|f| f.ast.clone());
            if let Some(ast) = ast {
                self.spreadsheet_mut()
                    .dependency_graph
                    .insert_formula_cell(*id, &ast);
            }
        }

        old_cell
    }

    /// Like set_cell but only updates the grid, not the dependency graph.
    /// Used during structural changes where the dep graph is rebuilt afterward.
    fn set_cell_grid_only(&mut self, id: &AbsoluteCellId, new_cell: Option<Cell>) -> Option<Cell> {
        let old_cell = self.spreadsheet.get_cell(id);
        match new_cell.as_ref() {
            Some(cell) => self.spreadsheet.insert_cell(id, cell.clone()),
            None => self.spreadsheet.remove_cell(id),
        }
        old_cell
    }

    // todo: simplify?

    fn record_cell_change(&mut self, id: AbsoluteCellId, new_cell: Option<Cell>) {
        let old_cell = self.set_cell(&id, new_cell.clone());
        self.batch.push(CellUpdate(id, old_cell, new_cell));
    }

    /// Parse string and insert appropriate cell.
    pub fn parse_and_insert_string(&mut self, _: &EngineGuard, id: AbsoluteCellId, input: &str) {
        if !input.starts_with('=') {
            let new = if let Ok(n) = input.parse::<Decimal>() {
                Cell::number(n)
            } else {
                Cell::text(input.to_string())
            };
            self.record_cell_change(id, Some(new));
            return;
        }

        // start parsing formula
        let formula_text = &input[1..];
        let lex_result = lex_formula(formula_text);
        // report any errors during lexing
        if let Some(err) = lex_result.errors().next() {
            let msg = format_lex_error(formula_text, err);
            self.record_cell_change(id, Some(Cell::error_with_input(msg, input.to_string())));
            return;
        }

        // return if empty
        let Some(tokens) = lex_result.into_output() else {
            return;
        };

        let parser_cell_id = GridCellId {
            row: id.row,
            col: id.col,
        };
        let mut state = FormulaState {
            names: &mut self.spreadsheet_mut().names,
            cell_id: parser_cell_id,
            expr_arena: Vec::new(),
            span_arena: Vec::new(),
        };
        let (parsed, parse_errs) = parse_formula(&tokens, formula_text.len(), &mut state);
        // report any errors during parsing (syntax, name not found)
        if !parse_errs.is_empty() {
            let msg = format_parse_error(formula_text, &parse_errs[0]);
            self.record_cell_change(id, Some(Cell::error_with_input(msg, input.to_string())));
            return;
        }
        let Some((ast, spans, _root_id)) = parsed else {
            return;
        };

        // create and store formula
        let formula = Formula {
            ast,
            spans,
            formula_string: input.to_string(),
        };
        let formula_id = self.spreadsheet_mut().formulas.insert(formula);

        // insert cell with formula reference and dependencies
        let new = Cell {
            defined_by_formula: Some(formula_id),
            val: CellValue::err(""),
            pending_dependencies: AtomicU32::new(0),
        };
        self.record_cell_change(id, Some(new));
    }

    pub fn insert_number(&mut self, _: &EngineGuard, id: AbsoluteCellId, n: Decimal) {
        self.record_cell_change(id, Some(Cell::number(n)));
    }

    pub fn insert_value(&mut self, _: &EngineGuard, id: AbsoluteCellId, val: CellValue) {
        let old = self.spreadsheet.get_cell(&id);
        self.spreadsheet.set_value_and_create_block(&id, val);
        let new = self.spreadsheet.get_cell(&id);
        self.batch.push(CellUpdate(id, old, new));
    }

    pub fn insert_shared_formula(
        &mut self,
        _: &EngineGuard,
        id: AbsoluteCellId,
        formula_id: FormulaId,
    ) {
        let new = Cell {
            defined_by_formula: Some(formula_id),
            val: CellValue::err(""),
            pending_dependencies: AtomicU32::new(0),
        };
        self.record_cell_change(id, Some(new));
    }

    pub fn delete(&mut self, _: &EngineGuard, id: AbsoluteCellId) {
        self.record_cell_change(id, None);
    }

    // recalculate all formulas affected by the given cell changes.
    // all changes should already be applied to the grid, and the dependency graph should be correct.
    fn post_cell_changes_hook(&mut self, changes: Vec<CellUpdate>) {
        self.debug.eval_number += 1;
        info!("Eval #{}", self.debug.eval_number);

        // step 1: discover all affected dependants and set their pending counters (sync)
        let init_time = Instant::now();
        let unique_cells: Vec<AbsoluteCellId> = changes
            .iter()
            .map(|CellUpdate(id, _, _)| *id)
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let changed_ranges = merge_cells_into_ranges(&unique_cells);
        {
            let sheets = self.spreadsheet.sheets.read();
            self.spreadsheet
                .dependency_graph
                .init_pending_counter_for_new_recalculation(&sheets, &changed_ranges);
        }
        let init_duration = init_time.elapsed();

        // step 2: collect cells with counter == 0 into the starting wave
        let mut wave: Vec<AbsoluteCellId> = Vec::new();
        {
            let sheets = self.spreadsheet.sheets.read();
            for &id in &unique_cells {
                let grid_id: GridCellId = (&id).into();
                if sheets[id.sheet_id as usize].get_pending_dependencies(&grid_id) == 0 {
                    wave.push(id);
                }
            }
        }

        // step 3: evaluate formulas in topological order using forte's scoped parallelism.
        //
        // each cell is spawned as a closure on the scope. external calls block the
        // individual worker (via mpsc recv on the IPC bridge), while other workers
        // continue evaluating independent cells.
        let eval_time = Instant::now();
        if !wave.is_empty() {
            let sp = &self.spreadsheet;
            EVAL_POOL.with_worker(|worker| {
                worker.scope(|scope| {
                    for cell in &wave {
                        let cell = *cell;
                        scope.spawn_on(worker, move |_: &forte::Worker| {
                            eval_cell(cell, sp, scope);
                        });
                    }
                });
            });
        }
        let eval_duration = eval_time.elapsed();

        // step 4: detect cells stuck in cycles (pending_dependencies still > 0 after eval)
        let cycle_time = Instant::now();
        {
            let sheets = self.spreadsheet.sheets.read();
            self.spreadsheet
                .dependency_graph
                .detect_cycles(&sheets, &changed_ranges);
        }
        let cycle_duration = cycle_time.elapsed();

        // step 5: update tables affected by changes
        // todo: optimize
        let table_time = Instant::now();
        let mut affected_tables: HashSet<u32> = HashSet::new();
        for CellUpdate(id, _, _) in &changes {
            if let Some((table_id, _)) = self.spreadsheet.find_table_containing_cell(id) {
                affected_tables.insert(table_id);
            }
        }
        for table_id in affected_tables {
            self.post_table_cells_change_hook(table_id);
        }

        info!("Eval (init pending counters) took: {:?}", init_duration);
        info!("Eval (running expressions) took: {:?}", eval_duration);
        info!("Eval (detecting cycles) took: {:?}", cycle_duration);
        info!("Eval (updating tables) took: {:?}", table_time.elapsed());
    }

    /// Finalize the batch: push to undo log, truncate redo history.
    pub fn end_batch(&mut self, _: EngineGuard) {
        if self.batch.is_empty() {
            return;
        }

        let changes = std::mem::take(&mut self.batch);
        let bounds = compute_bounds(&changes);
        let eval_changes = changes.clone();

        self.history.log.truncate(self.history.log_position);
        let id = self.history.next_log_id;
        self.history.next_log_id += 1;

        self.history.log.push(HistoryLogEntry::Update {
            id,
            changes,
            bounds,
        });
        self.history.log_position = self.history.log.len();

        self.post_cell_changes_hook(eval_changes);
    }

    fn apply_history_changes(&mut self, changes: &[CellUpdate], use_new: bool) {
        if use_new {
            for CellUpdate(id, _, new) in changes {
                self.set_cell(id, new.clone());
            }
        } else {
            // when undoing, walk backward to revert writes in exact reverse order
            for CellUpdate(id, old, _) in changes.iter().rev() {
                self.set_cell(id, old.clone());
            }
        }
    }

    pub fn undo(&mut self) -> Option<ChangeBounds> {
        if self.history.log_position == 0 {
            return None;
        }
        self.history.log_position -= 1;
        let entry = &self.history.log[self.history.log_position];
        let bounds = entry.bounds().clone();
        let changes = entry.changes().clone();

        self.apply_history_changes(&changes, false);
        self.post_cell_changes_hook(changes);
        Some(bounds)
    }

    pub fn redo(&mut self) -> Option<ChangeBounds> {
        if self.history.log_position >= self.history.log.len() {
            return None;
        }
        let entry = &self.history.log[self.history.log_position];
        let bounds = entry.bounds().clone();
        let changes = entry.changes().clone();
        self.history.log_position += 1;

        self.apply_history_changes(&changes, true);
        self.post_cell_changes_hook(changes);
        Some(bounds)
    }

    pub fn is_saved(&self) -> bool {
        let current_id = if self.history.log_position == 0 {
            Some(0)
        } else {
            Some(self.history.log[self.history.log_position - 1].id())
        };
        current_id == self.history.last_saved_log_id
    }

    fn post_table_cells_change_hook(&mut self, table_id: u32) {
        let table = match self.spreadsheet.tables.get(table_id) {
            Some(t) => t,
            None => return,
        };
        let sheet = table.sheet_id as usize;
        let col_start = table.body_start.col;
        let col_end = table.body_end.col;
        let row_start = table.body_start.row;
        let row_end = table.body_end.row;
        let proj_id = table.projection_id;

        let sp = self.spreadsheet_mut();
        let proj = sp.projections.get_mut(proj_id).unwrap();
        let num_cols = (col_end - col_start + 1) as usize;
        let sheets = sp.sheets.read();

        for col_idx in 0..num_cols {
            let col = col_start + col_idx as u32;
            let mut old_opts = std::mem::take(&mut proj.filter_options_per_column[col_idx]);

            // reset counts, rebuild from grid
            for opt in old_opts.values_mut() {
                opt.count = 0;
            }
            for row in row_start..=row_end {
                if let Some(val) = sheets[sheet].get_value(&GridCellId { row, col }) {
                    old_opts
                        .entry(val.clone())
                        .and_modify(|o| o.count += 1)
                        .or_insert_with(|| {
                            let id = proj.next_filter_option_id;
                            proj.next_filter_option_id += 1;
                            ProjectionFilterOption {
                                id,
                                selected: true,
                                count: 1,
                            }
                        });
                }
            }
            // remove options with count 0 (value no longer in grid)
            old_opts.retain(|_, o| o.count > 0);

            proj.filter_options_per_column[col_idx] = old_opts;
        }
    }

    fn collect_affected_cells_for_column_or_row_change(
        &mut self,
        spec: ColumnOrRowChangeSpec,
    ) -> Vec<AbsoluteCellId> {
        // references into shift range must be rewritten
        let shift_range = if spec.changing_row {
            CellRange::new(spec.sheet_id, spec.shift_start, 0, u32::MAX, u32::MAX)
        } else {
            CellRange::new(spec.sheet_id, 0, spec.shift_start, u32::MAX, u32::MAX)
        };

        // affected = dependants of shifted range + moved cells that depend on unshifted side
        let mut affected_cells = self
            .spreadsheet
            .dependency_graph
            .direct_dependants_for_range(shift_range);
        let mut seen_cells: HashSet<AbsoluteCellId> = affected_cells.iter().copied().collect();
        if spec.shift_start > 0 {
            let unshifted_range = if spec.changing_row {
                CellRange::new(spec.sheet_id, 0, 0, spec.shift_start - 1, u32::MAX)
            } else {
                CellRange::new(spec.sheet_id, 0, 0, u32::MAX, spec.shift_start - 1)
            };
            for dependant_id in self
                .spreadsheet
                .dependency_graph
                .direct_dependants_for_range(unshifted_range)
            {
                let is_moved_cell = dependant_id.sheet_id == spec.sheet_id
                    && if spec.changing_row {
                        dependant_id.row >= spec.shift_start
                    } else {
                        dependant_id.col >= spec.shift_start
                    };
                if is_moved_cell && seen_cells.insert(dependant_id) {
                    affected_cells.push(dependant_id);
                }
            }
        }

        // on remove, formulas that point directly into deleted axis must be rewritten too
        if let Some(deleted_axis) = spec.deleted_index {
            let deleted_range = if spec.changing_row {
                CellRange::new(spec.sheet_id, deleted_axis, 0, deleted_axis, u32::MAX)
            } else {
                CellRange::new(spec.sheet_id, 0, deleted_axis, u32::MAX, deleted_axis)
            };
            for dependant_id in self
                .spreadsheet
                .dependency_graph
                .direct_dependants_for_range(deleted_range)
            {
                if seen_cells.insert(dependant_id) {
                    affected_cells.push(dependant_id);
                }
            }
        }

        affected_cells
    }

    fn rewrite_formulas_for_column_or_row_change(
        &mut self,
        spec: ColumnOrRowChangeSpec,
        affected_cells: Vec<AbsoluteCellId>,
        changes: &mut Vec<CellUpdate>,
    ) {
        let mut replaced_formulas: HashMap<(FormulaId, u32), FormulaId> = HashMap::new();

        for dependant_id in affected_cells {
            let Some(dependant) = self.spreadsheet.get_cell(&dependant_id) else {
                continue;
            };
            let Some(formula_id) = dependant.defined_by_formula else {
                continue;
            };

            let source_axis = if spec.changing_row {
                dependant_id.row
            } else {
                dependant_id.col
            };
            let source_moves =
                dependant_id.sheet_id == spec.sheet_id && source_axis >= spec.shift_start;
            let cache_key = (formula_id, source_axis);
            let dependant_with_shifted_references;

            // if already replaced this formula, then just insert replacement formula's id
            if let Some(replacement_id) = replaced_formulas.get(&cache_key) {
                dependant_with_shifted_references = Cell {
                    defined_by_formula: Some(*replacement_id),
                    val: dependant.val.clone(),
                    pending_dependencies: AtomicU32::new(0),
                };
            } else {
                // otherwise, create new formula with shifted AST for structure update
                let mut formula = self
                    .spreadsheet
                    .formulas
                    .get(formula_id)
                    .expect("formula_id to exist")
                    .clone();

                for expr in formula.ast.iter_mut() {
                    let Expr::Atom(atom) = expr else {
                        continue;
                    };

                    let mut replace_with_error = false;

                    if let ExprAtom::Reference(reference) = atom {
                        let reference_range = reference.to_cell_range(&dependant_id);
                        if reference_range.sheet_id != spec.sheet_id {
                            continue;
                        }

                        if let Some(deleted_axis) = spec.deleted_index {
                            let points_to_deleted = if spec.changing_row {
                                reference_range.start_row <= deleted_axis
                                    && deleted_axis <= reference_range.end_row
                            } else {
                                reference_range.start_col <= deleted_axis
                                    && deleted_axis <= reference_range.end_col
                            };
                            if points_to_deleted {
                                replace_with_error = true;
                            }
                        }

                        if !replace_with_error {
                            // keep absolute and relative coordinates consistent after structure change
                            let source_base = if spec.changing_row {
                                dependant_id.row
                            } else {
                                dependant_id.col
                            };
                            let shifted_base = if source_moves {
                                match spec.change {
                                    ColumnOrRowChange::Insert => source_base.saturating_add(1),
                                    ColumnOrRowChange::Remove => source_base.saturating_sub(1),
                                }
                            } else {
                                source_base
                            };
                            let shift = |coord: &mut Coordinate| {
                                let source_abs = coord.to_index(source_base);
                                let shifted_abs = match spec.change {
                                    ColumnOrRowChange::Insert if source_abs >= spec.shift_start => {
                                        source_abs.saturating_add(1)
                                    }
                                    ColumnOrRowChange::Remove if source_abs >= spec.shift_start => {
                                        source_abs.saturating_sub(1)
                                    }
                                    _ => source_abs,
                                };
                                *coord = match coord {
                                    Coordinate::Absolute(_) => Coordinate::Absolute(shifted_abs),
                                    Coordinate::Relative(_) => Coordinate::Relative(
                                        (shifted_abs as i64 - shifted_base as i64)
                                            .clamp(i32::MIN as i64, i32::MAX as i64)
                                            as i32,
                                    ),
                                };
                            };

                            match reference {
                                Reference::Single { row, col, .. } => {
                                    if spec.changing_row {
                                        shift(row);
                                    } else {
                                        shift(col);
                                    }
                                }
                                Reference::Range {
                                    start_row,
                                    start_col,
                                    end_row,
                                    end_col,
                                    ..
                                } => {
                                    if spec.changing_row {
                                        shift(start_row);
                                        shift(end_row);
                                    } else {
                                        shift(start_col);
                                        shift(end_col);
                                    }
                                }
                            }
                        }
                    }

                    if replace_with_error {
                        // keep formula shape, but mark deleted reference as error atom
                        *atom = ExprAtom::InvalidReferenceError("#REF!".into());
                    }
                }

                let replacement_id = self.spreadsheet_mut().formulas.insert(formula);
                replaced_formulas.insert(cache_key, replacement_id);
                dependant_with_shifted_references = Cell {
                    defined_by_formula: Some(replacement_id),
                    val: dependant.val.clone(),
                    pending_dependencies: AtomicU32::new(0),
                };
            }

            let old = self.set_cell_grid_only(
                &dependant_id,
                Some(dependant_with_shifted_references.clone()),
            );
            changes.push(CellUpdate(
                dependant_id,
                old,
                Some(dependant_with_shifted_references),
            ));
        }
    }

    fn move_cells_for_column_or_row_change(
        &mut self,
        spec: ColumnOrRowChangeSpec,
        changes: &mut Vec<CellUpdate>,
    ) {
        let sheets = self.spreadsheet.sheets.read();
        let max_row = sheets[spec.sheet_id as usize].find_biggest_row();
        let max_col = sheets[spec.sheet_id as usize].find_biggest_column();
        let sheet = &sheets[spec.sheet_id as usize];
        let mut moved = Vec::new();
        let mut deleted = Vec::new();

        if spec.changing_row {
            if spec.shift_start <= max_row {
                sheet.for_each_cell_in_range(spec.shift_start, 0, max_row, max_col, |id, cell| {
                    moved.push((id, cell.clone()));
                });
            } else if spec.change == ColumnOrRowChange::Remove && spec.index <= max_row {
                sheet.for_each_cell_in_range(spec.index, 0, spec.index, max_col, |id, _| {
                    deleted.push(id);
                });
            }
        } else if spec.shift_start <= max_col {
            sheet.for_each_cell_in_range(0, spec.shift_start, max_row, max_col, |id, cell| {
                moved.push((id, cell.clone()));
            });
        } else if spec.change == ColumnOrRowChange::Remove && spec.index <= max_col {
            sheet.for_each_cell_in_range(0, spec.index, max_row, spec.index, |id, _| {
                deleted.push(id);
            });
        }
        drop(sheets);

        match (spec.changing_row, spec.change) {
            (true, ColumnOrRowChange::Insert) => {
                moved.sort_unstable_by(|(a, _), (b, _)| {
                    b.row.cmp(&a.row).then_with(|| a.col.cmp(&b.col))
                });
            }
            (false, ColumnOrRowChange::Insert) => {
                moved.sort_unstable_by(|(a, _), (b, _)| {
                    b.col.cmp(&a.col).then_with(|| a.row.cmp(&b.row))
                });
            }
            (true, ColumnOrRowChange::Remove) => {
                moved.sort_unstable_by(|(a, _), (b, _)| {
                    a.row.cmp(&b.row).then_with(|| a.col.cmp(&b.col))
                });
            }
            (false, ColumnOrRowChange::Remove) => {
                moved.sort_unstable_by(|(a, _), (b, _)| {
                    a.col.cmp(&b.col).then_with(|| a.row.cmp(&b.row))
                });
            }
        }

        for deleted_id in deleted {
            let deleted_id = AbsoluteCellId {
                sheet_id: spec.sheet_id,
                row: deleted_id.row,
                col: deleted_id.col,
            };
            let old = self.set_cell_grid_only(&deleted_id, None);
            changes.push(CellUpdate(deleted_id, old, None));
        }

        for (source_id, cell) in moved {
            let source_id = AbsoluteCellId {
                sheet_id: spec.sheet_id,
                row: source_id.row,
                col: source_id.col,
            };
            let dest_id = match (spec.changing_row, spec.change) {
                (true, ColumnOrRowChange::Insert) => AbsoluteCellId {
                    sheet_id: spec.sheet_id,
                    row: source_id.row + 1,
                    col: source_id.col,
                },
                (false, ColumnOrRowChange::Insert) => AbsoluteCellId {
                    sheet_id: spec.sheet_id,
                    row: source_id.row,
                    col: source_id.col + 1,
                },
                (true, ColumnOrRowChange::Remove) => AbsoluteCellId {
                    sheet_id: spec.sheet_id,
                    row: source_id.row - 1,
                    col: source_id.col,
                },
                (false, ColumnOrRowChange::Remove) => AbsoluteCellId {
                    sheet_id: spec.sheet_id,
                    row: source_id.row,
                    col: source_id.col - 1,
                },
            };

            let old_source = self.set_cell_grid_only(&source_id, None);
            let old_dest = self.set_cell_grid_only(&dest_id, Some(cell.clone()));
            self.spreadsheet_mut()
                .names
                .move_cell_name(&source_id, &dest_id);

            changes.push(CellUpdate(dest_id, old_dest, Some(cell)));
            changes.push(CellUpdate(source_id, old_source, None));
        }
        if spec.change == ColumnOrRowChange::Remove {
            self.spreadsheet.sheets.write()[spec.sheet_id as usize].refresh_bounds();
        }
    }

    fn apply_column_or_row_change(&mut self, spec: ColumnOrRowChangeSpec) -> Vec<CellUpdate> {
        let collect_time = Instant::now();
        let affected_cells = self.collect_affected_cells_for_column_or_row_change(spec);
        let dependants_duration = collect_time.elapsed();

        let mut changes = Vec::new();
        let formula_time = Instant::now();
        self.rewrite_formulas_for_column_or_row_change(spec, affected_cells, &mut changes);
        let formula_duration = formula_time.elapsed();

        let move_time = Instant::now();
        self.move_cells_for_column_or_row_change(spec, &mut changes);
        let move_duration = move_time.elapsed();

        // dep graph is stale (grid-only updates above skipped it).
        // targeted rebuild: remove old edges for affected cells, re-insert from current grid.
        let rebuild_time = Instant::now();
        let affected: HashSet<AbsoluteCellId> =
            changes.iter().map(|CellUpdate(id, _, _)| *id).collect();
        for &id in &affected {
            self.spreadsheet_mut()
                .dependency_graph
                .remove_formula_cell(id);
        }
        for &id in &affected {
            if let Some(cell) = self.spreadsheet.get_cell(&id) {
                if let Some(formula_id) = cell.defined_by_formula {
                    let ast = self
                        .spreadsheet
                        .formulas
                        .get(formula_id)
                        .map(|f| f.ast.clone());
                    if let Some(ast) = ast {
                        self.spreadsheet_mut()
                            .dependency_graph
                            .insert_formula_cell(id, &ast);
                    }
                }
            }
        }
        let rebuild_duration = rebuild_time.elapsed();

        debug!(
            "{} (dependencies & dependants) took: {:?}",
            spec.operation_name(),
            dependants_duration
        );
        debug!(
            "{} (rewriting formulas) took: {:?}",
            spec.operation_name(),
            formula_duration
        );
        debug!(
            "{} (moving cells) took: {:?}",
            spec.operation_name(),
            move_duration
        );
        debug!(
            "{} (rebuilding dep graph) took: {:?}",
            spec.operation_name(),
            rebuild_duration
        );

        changes
    }

    fn push_structural_history_entry(
        &mut self,
        spec: ColumnOrRowChangeSpec,
        changes: Vec<CellUpdate>,
    ) {
        let bounds = if changes.is_empty() {
            if spec.changing_row {
                ChangeBounds {
                    min_row: spec.index,
                    max_row: spec.index,
                    min_col: 0,
                    max_col: 0,
                }
            } else {
                ChangeBounds {
                    min_row: 0,
                    max_row: 0,
                    min_col: spec.index,
                    max_col: spec.index,
                }
            }
        } else {
            compute_bounds(&changes)
        };

        self.history.log.truncate(self.history.log_position);
        let id = self.history.next_log_id;
        self.history.next_log_id += 1;

        let entry = match (spec.change, spec.changing_row) {
            (ColumnOrRowChange::Insert, true) => HistoryLogEntry::InsertRow {
                id,
                changes,
                bounds,
            },
            (ColumnOrRowChange::Insert, false) => HistoryLogEntry::InsertColumn {
                id,
                changes,
                bounds,
            },
            (ColumnOrRowChange::Remove, true) => HistoryLogEntry::RemoveRow {
                id,
                changes,
                bounds,
            },
            (ColumnOrRowChange::Remove, false) => HistoryLogEntry::RemoveColumn {
                id,
                changes,
                bounds,
            },
        };
        self.history.log.push(entry);
        self.history.log_position = self.history.log.len();
    }

    pub fn insert_column_or_row(
        &mut self,
        sheet_id: u32,
        inserting_row: bool,
        index: u32,
        include_index: bool,
    ) {
        // one path for insert, split by row/column and include_index inside spec
        let spec = ColumnOrRowChangeSpec::for_insert(sheet_id, inserting_row, index, include_index);
        let changes = self.apply_column_or_row_change(spec);
        self.push_structural_history_entry(spec, changes);
        self.history.last_saved_log_id = None;
    }

    pub fn remove_column_or_row(&mut self, sheet_id: u32, removing_row: bool, index: u32) {
        // one path for remove, split by row/column inside spec
        let spec = ColumnOrRowChangeSpec::for_remove(sheet_id, removing_row, index);
        let changes = self.apply_column_or_row_change(spec);
        self.push_structural_history_entry(spec, changes.clone());
        if !changes.is_empty() {
            self.post_cell_changes_hook(changes);
        }
        self.history.last_saved_log_id = None;
    }

    /// Sort table by column by reordering the current projected rows.
    pub fn sort_table_column(
        &mut self,
        table_id: u32,
        sort_col: u32,
        desc: bool,
    ) -> Result<(), String> {
        let table = self
            .spreadsheet
            .tables
            .get(table_id)
            .ok_or("Table not found")?;
        let sheet = table.sheet_id as usize;
        let proj_id = table.projection_id;

        let sp = self.spreadsheet_mut();
        let sheets = sp.sheets.read();
        let proj = sp.projections.get_mut(proj_id).unwrap();
        proj.projected_rows.sort_by(|&a, &b| {
            let va = sheets[sheet].get_value(&GridCellId {
                row: a,
                col: sort_col,
            });
            let vb = sheets[sheet].get_value(&GridCellId {
                row: b,
                col: sort_col,
            });
            // empty cells always sink to the bottom, regardless of sort direction
            match (va, vb) {
                (None, None) => std::cmp::Ordering::Equal,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (Some(_), None) => std::cmp::Ordering::Less,
                (Some(va), Some(vb)) => {
                    let ord = match (va, vb) {
                        (CellValue::Number(n1), CellValue::Number(n2)) => n1.cmp(&n2),
                        (CellValue::Text(t1), CellValue::Text(t2)) => t1.cmp(&t2),
                        (CellValue::Number(_), _) => std::cmp::Ordering::Less,
                        (_, CellValue::Number(_)) => std::cmp::Ordering::Greater,
                        _ => std::cmp::Ordering::Equal,
                    };
                    if desc {
                        ord.reverse()
                    } else {
                        ord
                    }
                }
            }
        });
        proj.active = true;
        Ok(())
    }

    /// Toggle a filter value for a table column. Returns hidden_rows_count.
    pub fn filter_table_column(
        &mut self,
        table_id: u32,
        filter_col: u32,
        filter_option_id: u32,
    ) -> Result<u32, String> {
        let table = self
            .spreadsheet
            .tables
            .get(table_id)
            .ok_or("Table not found")?;
        let col_start = table.body_start.col;
        let proj_id = table.projection_id;
        let proj = self.spreadsheet_mut().projections.get_mut(proj_id).unwrap();
        let col_idx = (filter_col - col_start) as usize;

        if filter_option_id == 0 {
            proj.filter_show_blanks[col_idx] = !proj.filter_show_blanks[col_idx];
        } else if let Some(opt) = proj.filter_options_per_column[col_idx]
            .values_mut()
            .find(|o| o.id == filter_option_id)
        {
            opt.selected = !opt.selected;
        }

        self.rebuild_table_projection(table_id)
    }

    /// Select all filter options for a table column. Returns hidden_rows_count.
    pub fn select_all_column_filters(
        &mut self,
        table_id: u32,
        filter_col: u32,
    ) -> Result<u32, String> {
        let table = self
            .spreadsheet
            .tables
            .get(table_id)
            .ok_or("Table not found")?;
        let col_start = table.body_start.col;
        let proj_id = table.projection_id;
        let proj = self.spreadsheet_mut().projections.get_mut(proj_id).unwrap();
        let col_idx = (filter_col - col_start) as usize;

        proj.filter_show_blanks[col_idx] = true;
        for opt in proj.filter_options_per_column[col_idx].values_mut() {
            opt.selected = true;
        }

        self.rebuild_table_projection(table_id)
    }

    /// Clear all filter options for a table column. Returns hidden_rows_count.
    pub fn clear_all_column_filters(
        &mut self,
        table_id: u32,
        filter_col: u32,
    ) -> Result<u32, String> {
        let table = self
            .spreadsheet
            .tables
            .get(table_id)
            .ok_or("Table not found")?;
        let col_start = table.body_start.col;
        let proj_id = table.projection_id;
        let proj = self.spreadsheet_mut().projections.get_mut(proj_id).unwrap();
        let col_idx = (filter_col - col_start) as usize;

        proj.filter_show_blanks[col_idx] = false;
        for opt in proj.filter_options_per_column[col_idx].values_mut() {
            opt.selected = false;
        }

        self.rebuild_table_projection(table_id)
    }

    /// Rebuild projected_rows for a table from current filter state. Returns hidden_rows_count.
    fn rebuild_table_projection(&mut self, table_id: u32) -> Result<u32, String> {
        let table = self
            .spreadsheet
            .tables
            .get(table_id)
            .ok_or("Table not found")?;
        let sheet = table.sheet_id as usize;
        let col_start = table.body_start.col;
        let body_start_row = table.body_start.row;
        let body_end_row = table.body_end.row;
        let proj_id = table.projection_id;

        let sp = self.spreadsheet_mut();
        let sheets = sp.sheets.read();
        let proj = sp.projections.get_mut(proj_id).unwrap();

        // rebuild projected_rows from all body rows with current filters
        let mut rows: Vec<u32> = (body_start_row..=body_end_row).collect();
        for (i, filter_opts) in proj.filter_options_per_column.iter().enumerate() {
            let col_blanks = proj.filter_show_blanks[i];
            if col_blanks && filter_opts.values().all(|o| o.selected) {
                continue;
            }
            let col = col_start + i as u32;
            rows.retain(
                |&row| match sheets[sheet].get_value(&GridCellId { row, col }) {
                    Some(v) => filter_opts.get(&v).map_or(false, |o| o.selected),
                    None => col_blanks,
                },
            );
        }

        let total = (body_end_row - body_start_row + 1) as u32;
        proj.hidden_rows_count = total - rows.len() as u32;
        proj.projected_rows = rows;
        proj.active = proj.filter_show_blanks.iter().any(|&b| !b)
            || proj
                .filter_options_per_column
                .iter()
                .any(|col| col.values().any(|o| !o.selected));
        Ok(proj.hidden_rows_count)
    }

    /// Reset engine with an empty spreadsheet.
    pub fn create_empty_spreadsheet(&mut self) {
        self.spreadsheet = Arc::new(Spreadsheet::new());
        self.history = History::new();
        self.batch.clear();
    }

    /// Load a spreadsheet from disk and reset engine state.
    /// Returns the UI decorations JSON string (empty if old format).
    pub fn open_spreadsheet(&mut self, path: &str) -> io::Result<String> {
        let (spreadsheet, decorations) = file_api::load(path)?;
        self.spreadsheet = Arc::new(spreadsheet);
        for sheet in self.spreadsheet.sheets.write().iter_mut() {
            sheet.refresh_bounds();
        }

        let dependency_graph_time = Instant::now();
        self.spreadsheet_mut().rebuild_dependency_graph();
        info!(
            "open_spreadsheet: dependency graph built in {:?}",
            dependency_graph_time.elapsed()
        );

        self.history = History::new();
        self.batch.clear();
        self.mark_saved();
        Ok(decorations)
    }

    /// Save the current spreadsheet and UI decorations to disk.
    pub fn save_spreadsheet(&mut self, path: &str, decorations_json: &str) -> io::Result<()> {
        file_api::save(&self.spreadsheet, decorations_json, path)?;
        self.mark_saved();
        Ok(())
    }

    pub fn mark_saved(&mut self) {
        self.history.last_saved_log_id = if self.history.log_position == 0 {
            Some(0)
        } else {
            Some(self.history.log[self.history.log_position - 1].id())
        };
    }
}

/// Resolve an ExprAtom to a Decimal number. If it's a single cell reference, look up the cell value.
fn resolve_number(
    source_cell: &AbsoluteCellId,
    expr: &ExprAtom,
    sheets: &Arc<RwLock<Sheets>>,
) -> Result<Decimal, EvalError> {
    match expr {
        ExprAtom::Number(n) => Ok(*n),
        ExprAtom::InvalidReferenceError(msg) => Err(EvalError::Error(msg.clone())),
        ExprAtom::Reference(r) => match r {
            Reference::Single { sheet_id, row, col } => {
                let row = row.to_index(source_cell.row);
                let col = col.to_index(source_cell.col);
                let gid = GridCellId { row, col };
                match sheets.read()[*sheet_id as usize].get_value(&gid) {
                    Some(CellValue::Number(n)) => Ok(n),
                    Some(CellValue::Text(_)) => Err(EvalError::TypeError {
                        expected: AtomType::Number,
                        got: AtomType::Text,
                        span: None,
                    }),
                    Some(CellValue::Bool(_)) => Err(EvalError::TypeError {
                        expected: AtomType::Number,
                        got: AtomType::Bool,
                        span: None,
                    }),
                    Some(CellValue::Error(msg, _)) => Err(EvalError::Error(msg.to_string())),
                    None => Ok(Decimal::ZERO),
                }
            }
            Reference::Range { .. } => Err(EvalError::TypeError {
                expected: AtomType::Number,
                got: AtomType::Reference,
                span: None,
            }),
        },
        _ => Err(EvalError::TypeError {
            expected: AtomType::Number,
            got: match expr {
                ExprAtom::Bool(_) => AtomType::Bool,
                ExprAtom::Text(_) => AtomType::Text,
                ExprAtom::InvalidReferenceError(_) => AtomType::InvalidReferenceError,
                ExprAtom::Function(_) => AtomType::Function,
                _ => unreachable!(),
            },
            span: None,
        }),
    }
}

fn resolve_bool(
    source_cell: &AbsoluteCellId,
    expr: &ExprAtom,
    sheets: &Arc<RwLock<Sheets>>,
) -> Result<bool, EvalError> {
    match expr {
        ExprAtom::Bool(b) => Ok(*b),
        ExprAtom::InvalidReferenceError(msg) => Err(EvalError::Error(msg.clone())),
        ExprAtom::Reference(r) => match r {
            Reference::Single { sheet_id, row, col } => {
                let row = row.to_index(source_cell.row);
                let col = col.to_index(source_cell.col);
                let gid = GridCellId { row, col };
                match sheets.read()[*sheet_id as usize].get_value(&gid) {
                    Some(CellValue::Bool(b)) => Ok(b),
                    Some(CellValue::Number(_)) => Err(EvalError::TypeError {
                        expected: AtomType::Bool,
                        got: AtomType::Number,
                        span: None,
                    }),
                    Some(CellValue::Text(_)) => Err(EvalError::TypeError {
                        expected: AtomType::Bool,
                        got: AtomType::Text,
                        span: None,
                    }),
                    Some(CellValue::Error(msg, _)) => Err(EvalError::Error(msg.to_string())),
                    None => Ok(false),
                }
            }
            Reference::Range { .. } => Err(EvalError::TypeError {
                expected: AtomType::Bool,
                got: AtomType::Reference,
                span: None,
            }),
        },
        _ => Err(EvalError::TypeError {
            expected: AtomType::Bool,
            got: match expr {
                ExprAtom::Number(_) => AtomType::Number,
                ExprAtom::Text(_) => AtomType::Text,
                ExprAtom::Function(_) => AtomType::Function,
                _ => unreachable!(),
            },
            span: None,
        }),
    }
}

/// Resolve a final ExprAtom to CellValue, resolving references.
fn resolve_to_cell_value(
    source_cell: &AbsoluteCellId,
    expr: ExprAtom,
    sheets: &Arc<RwLock<Sheets>>,
) -> CellValue {
    match expr {
        ExprAtom::Number(n) => CellValue::Number(n),
        ExprAtom::Text(s) => CellValue::Text(s.into()),
        ExprAtom::Bool(b) => CellValue::Bool(b),
        ExprAtom::InvalidReferenceError(s) => CellValue::err(s),
        ExprAtom::Reference(Reference::Single { sheet_id, row, col }) => {
            let row = row.to_index(source_cell.row);
            let col = col.to_index(source_cell.col);
            let gid = GridCellId { row, col };
            sheets.read()[sheet_id as usize]
                .get_value(&gid)
                .unwrap_or(CellValue::Number(Decimal::ZERO))
        }
        _ => CellValue::Text(format!("{:?}", expr).into()),
    }
}

/// Resolve an ExprAtom to a range iterator. Returns (sheet_id, start_row, start_col, end_row, end_col).
fn resolve_range(
    source_cell: &AbsoluteCellId,
    expr: &ExprAtom,
) -> Result<(SheetId, u32, u32, u32, u32), EvalError> {
    match expr {
        ExprAtom::InvalidReferenceError(msg) => Err(EvalError::Error(msg.clone())),
        ExprAtom::Reference(Reference::Range {
            sheet_id,
            start_row,
            start_col,
            end_row,
            end_col,
        }) => {
            let sr = start_row.to_index(source_cell.row);
            let sc = start_col.to_index(source_cell.col);
            let er = end_row.to_index(source_cell.row);
            let ec = end_col.to_index(source_cell.col);
            Ok((*sheet_id, sr.min(er), sc.min(ec), sr.max(er), sc.max(ec)))
        }
        _ => Err(EvalError::TypeError {
            expected: AtomType::Reference,
            got: match expr {
                ExprAtom::Bool(_) => AtomType::Bool,
                ExprAtom::Number(_) => AtomType::Number,
                ExprAtom::Text(_) => AtomType::Text,
                ExprAtom::InvalidReferenceError(_) => AtomType::InvalidReferenceError,
                ExprAtom::Function(_) => AtomType::Function,
                ExprAtom::Reference(Reference::Single { .. }) => AtomType::Reference,
                _ => unreachable!(),
            },
            span: None,
        }),
    }
}

/// Parallel sum+count over a cell range. Uses forte::join to binary-split rows
/// across the thread pool. The heartbeat scheduler decides when to actually
/// parallelize — small ranges run sequentially with no overhead.
const PARALLEL_RANGE_THRESHOLD: u64 = 1_000_000;

fn parallel_sum_count(
    sp: &Arc<Spreadsheet>,
    sheet_id: u32,
    sr: u32,
    sc: u32,
    er: u32,
    ec: u32,
) -> (Decimal, u64) {
    let total_cells = (er - sr + 1) as u64 * (ec - sc + 1) as u64;
    if total_cells <= PARALLEL_RANGE_THRESHOLD {
        let sheets = sp.sheets.read();
        let sheet = &sheets[sheet_id as usize];
        let mut sum = Decimal::ZERO;
        let mut count = 0u64;
        sheet.for_each_value_in_range(sr, sc, er, ec, |val| {
            if let CellValue::Number(n) = val {
                sum += *n;
                count += 1;
            }
        });
        return (sum, count);
    }

    fn split(
        sp: &Arc<Spreadsheet>,
        sheet_id: u32,
        sr: u32,
        sc: u32,
        er: u32,
        ec: u32,
        worker: &forte::Worker,
    ) -> (Decimal, u64) {
        let rows = (er - sr + 1) as u64;
        let cols = (ec - sc + 1) as u64;
        if rows * cols <= 64_000 {
            let sheets = sp.sheets.read();
            let sheet = &sheets[sheet_id as usize];
            let mut sum = Decimal::ZERO;
            let mut count = 0u64;
            sheet.for_each_value_in_range(sr, sc, er, ec, |val| {
                if let CellValue::Number(n) = val {
                    sum += *n;
                    count += 1;
                }
            });
            return (sum, count);
        }
        let mid = (sr + er) / 2;
        let ((ls, lc), (rs, rc)) = worker.join(
            |w| split(sp, sheet_id, sr, sc, mid, ec, w),
            |w| split(sp, sheet_id, mid + 1, sc, er, ec, w),
        );
        (ls + rs, lc + rc)
    }

    forte::Worker::with_current(|w| split(sp, sheet_id, sr, sc, er, ec, w.unwrap()))
}

// convert an eval-store atom to an ExternArg for JS dispatch
// references are sent as coordinates — the JS dispatcher resolves them
// via the resolve_reference command (which reads sheets directly, no lock conflict)
fn resolve_extern_arg(atom: &ExprAtom, source_cell: &AbsoluteCellId) -> ExternArg {
    match atom {
        ExprAtom::Number(n) => ExternArg::Number(n.to_string()),
        ExprAtom::Text(s) => ExternArg::Text(s.to_string()),
        ExprAtom::Bool(b) => ExternArg::Boolean(*b),
        ExprAtom::Reference(Reference::Single { sheet_id, row, col }) => ExternArg::SingleRef {
            sheet_id: *sheet_id,
            row: row.to_index(source_cell.row),
            col: col.to_index(source_cell.col),
        },
        ExprAtom::Reference(Reference::Range {
            sheet_id,
            start_row,
            start_col,
            end_row,
            end_col,
        }) => ExternArg::RangeRef {
            sheet_id: *sheet_id,
            start_row: start_row.to_index(source_cell.row),
            start_col: start_col.to_index(source_cell.col),
            end_row: end_row.to_index(source_cell.row),
            end_col: end_col.to_index(source_cell.col),
        },
        _ => ExternArg::Text(String::new()),
    }
}

/// Evaluate a single formula's AST, returning the computed CellValue.
fn eval_formula(
    ast: &[Expr],
    spans: &[(u32, u32)],
    source_cell: &AbsoluteCellId,
    sp: &Arc<Spreadsheet>,
    eval_store: &mut Vec<ExprAtom>,
) -> Result<CellValue, EvalError> {
    eval_store.clear();
    let sheets = &sp.sheets;
    let external_functions = &sp.external_functions;
    let sp_id = |id: &ExprId| spans.get(*id as usize).copied();

    for expr in ast {
        let res = match expr {
            Expr::Atom(atom) => atom.clone(),
            Expr::Negate(id) => {
                let n = resolve_number(source_cell, &eval_store[*id as usize], sheets)
                    .map_err(|e| e.with_span(sp_id(id)))?;
                ExprAtom::Number(-n)
            }
            Expr::Add(a_id, b_id) => {
                let a = resolve_number(source_cell, &eval_store[*a_id as usize], sheets)
                    .map_err(|e| e.with_span(sp_id(a_id)))?;
                let b = resolve_number(source_cell, &eval_store[*b_id as usize], sheets)
                    .map_err(|e| e.with_span(sp_id(b_id)))?;
                ExprAtom::Number(a + b)
            }
            Expr::Subtract(a_id, b_id) => {
                let a = resolve_number(source_cell, &eval_store[*a_id as usize], sheets)
                    .map_err(|e| e.with_span(sp_id(a_id)))?;
                let b = resolve_number(source_cell, &eval_store[*b_id as usize], sheets)
                    .map_err(|e| e.with_span(sp_id(b_id)))?;
                ExprAtom::Number(a - b)
            }
            Expr::Multiply(a_id, b_id) => {
                let a = resolve_number(source_cell, &eval_store[*a_id as usize], sheets)
                    .map_err(|e| e.with_span(sp_id(a_id)))?;
                let b = resolve_number(source_cell, &eval_store[*b_id as usize], sheets)
                    .map_err(|e| e.with_span(sp_id(b_id)))?;
                ExprAtom::Number(a * b)
            }
            Expr::Divide(a_id, b_id) => {
                let a = resolve_number(source_cell, &eval_store[*a_id as usize], sheets)
                    .map_err(|e| e.with_span(sp_id(a_id)))?;
                let b = resolve_number(source_cell, &eval_store[*b_id as usize], sheets)
                    .map_err(|e| e.with_span(sp_id(b_id)))?;
                if b.is_zero() {
                    return Err(EvalError::DivisionByZero { span: sp_id(b_id) });
                }
                ExprAtom::Number(a / b)
            }
            Expr::Equal(a_id, b_id) => {
                let a = resolve_number(source_cell, &eval_store[*a_id as usize], sheets)
                    .map_err(|e| e.with_span(sp_id(a_id)))?;
                let b = resolve_number(source_cell, &eval_store[*b_id as usize], sheets)
                    .map_err(|e| e.with_span(sp_id(b_id)))?;
                ExprAtom::Bool(a == b)
            }
            Expr::GreaterThan(a_id, b_id) => {
                let a = resolve_number(source_cell, &eval_store[*a_id as usize], sheets)
                    .map_err(|e| e.with_span(sp_id(a_id)))?;
                let b = resolve_number(source_cell, &eval_store[*b_id as usize], sheets)
                    .map_err(|e| e.with_span(sp_id(b_id)))?;
                ExprAtom::Bool(a > b)
            }
            Expr::LessThan(a_id, b_id) => {
                let a = resolve_number(source_cell, &eval_store[*a_id as usize], sheets)
                    .map_err(|e| e.with_span(sp_id(a_id)))?;
                let b = resolve_number(source_cell, &eval_store[*b_id as usize], sheets)
                    .map_err(|e| e.with_span(sp_id(b_id)))?;
                ExprAtom::Bool(a < b)
            }
            Expr::Sum(range_id) => {
                let (sheet_id, sr, sc, er, ec) =
                    resolve_range(source_cell, &eval_store[*range_id as usize])?;
                let (sum, _) = parallel_sum_count(sp, sheet_id, sr, sc, er, ec);
                ExprAtom::Number(sum)
            }
            Expr::Avg(range_id) => {
                let (sheet_id, sr, sc, er, ec) =
                    resolve_range(source_cell, &eval_store[*range_id as usize])?;
                let (sum, count) = parallel_sum_count(sp, sheet_id, sr, sc, er, ec);
                if count == 0 {
                    ExprAtom::Number(Decimal::ZERO)
                } else {
                    ExprAtom::Number(sum / Decimal::from(count))
                }
            }
            Expr::Min(range_id) => {
                let (sheet_id, sr, sc, er, ec) =
                    resolve_range(source_cell, &eval_store[*range_id as usize])?;
                let sheets = sp.sheets.read();
                let sheet = &sheets[sheet_id as usize];
                let mut min: Option<Decimal> = None;
                sheet.for_each_value_in_range(sr, sc, er, ec, |val| {
                    if let CellValue::Number(n) = val {
                        min = Some(min.map_or(*n, |m: Decimal| m.min(*n)));
                    }
                });
                ExprAtom::Number(min.unwrap_or(Decimal::ZERO))
            }
            Expr::Max(range_id) => {
                let (sheet_id, sr, sc, er, ec) =
                    resolve_range(source_cell, &eval_store[*range_id as usize])?;
                let sheets = sp.sheets.read();
                let sheet = &sheets[sheet_id as usize];
                let mut max: Option<Decimal> = None;
                sheet.for_each_value_in_range(sr, sc, er, ec, |val| {
                    if let CellValue::Number(n) = val {
                        max = Some(max.map_or(*n, |m: Decimal| m.max(*n)));
                    }
                });
                ExprAtom::Number(max.unwrap_or(Decimal::ZERO))
            }
            Expr::Count(range_id, value_id) => {
                let (sheet_id, sr, sc, er, ec) =
                    resolve_range(source_cell, &eval_store[*range_id as usize])?;
                let target = resolve_number(source_cell, &eval_store[*value_id as usize], sheets)
                    .map_err(|e| e.with_span(sp_id(value_id)))?;
                let sheets_guard = sp.sheets.read();
                let sheet = &sheets_guard[sheet_id as usize];
                let mut count = 0u64;
                sheet.for_each_value_in_range(sr, sc, er, ec, |val| {
                    if let CellValue::Number(n) = val {
                        if *n == target {
                            count += 1;
                        }
                    }
                });
                ExprAtom::Number(Decimal::from(count))
            }
            Expr::If(cond_id, then_id, else_id) => {
                let cond = resolve_bool(source_cell, &eval_store[*cond_id as usize], sheets)
                    .map_err(|e| e.with_span(sp_id(cond_id)))?;
                if cond {
                    eval_store[*then_id as usize].clone()
                } else {
                    eval_store[*else_id as usize].clone()
                }
            }
            Expr::ExtrnalFunctionCall { func_id, args } => {
                let Some(func) = external_functions.get(*func_id) else {
                    return Err(EvalError::Error(format!("unknown function id {}", func_id)));
                };
                if args.len() != func.args.len() {
                    return Err(EvalError::Error(format!(
                        "{}(): expected {} args, got {}",
                        func.name,
                        func.args.len(),
                        args.len()
                    )));
                }

                let js_args: Vec<ExternArg> = args
                    .iter()
                    .map(|expr_id| resolve_extern_arg(&eval_store[*expr_id as usize], source_cell))
                    .collect();

                let cell_value = call_extern_js_function(&func.name, js_args)?;
                match cell_value {
                    CellValue::Number(n) => ExprAtom::Number(n),
                    CellValue::Text(s) => ExprAtom::Text(s.to_string()),
                    CellValue::Bool(b) => ExprAtom::Bool(b),
                    CellValue::Error(s, _) => {
                        return Err(EvalError::Error(s.to_string()));
                    }
                }
            }
        };
        eval_store.push(res);
    }

    let value = resolve_to_cell_value(source_cell, eval_store.pop().unwrap(), &sp.sheets);

    Ok(value)
}

// evaluate a single cell, write result to grid, and spawn ready dependants onto the scope.
// external calls block this worker via the IPC bridge (mpsc channel), other workers continue.
fn eval_cell<'a>(
    cell_id: AbsoluteCellId,
    sp: &'a Arc<Spreadsheet>,
    scope: &'a forte::Scope<'a, '_>,
) {
    let mut eval_store = Vec::new();
    let formula_id = sp.get_cell(&cell_id).and_then(|c| c.defined_by_formula);
    if let Some(formula_id) = formula_id {
        if let Some(formula) = sp.formulas.get(formula_id) {
            let val =
                match eval_formula(&formula.ast, &formula.spans, &cell_id, sp, &mut eval_store) {
                    Ok(v) => v,
                    Err(EvalError::Error(msg)) => CellValue::err(msg),
                    Err(EvalError::TypeError {
                        expected,
                        got,
                        span,
                    }) => {
                        let msg = format!("type error: expected {expected}, got {got}");
                        let formatted = format_eval_error(&formula.formula_string, &msg, span);
                        CellValue::err(formatted)
                    }
                    Err(EvalError::DivisionByZero { span }) => {
                        let formatted =
                            format_eval_error(&formula.formula_string, "division by zero", span);
                        CellValue::err(formatted)
                    }
                };
            sp.set_value(&cell_id, val);
        }
    }
    let ready = sp
        .dependency_graph
        .decrease_pending_counter(&sp.sheets.read(), CellRange::single(cell_id));
    for r in ready {
        scope.spawn(move |_: &forte::Worker| eval_cell(r, sp, scope));
    }
}

fn compute_bounds(changes: &[CellUpdate]) -> ChangeBounds {
    let mut min_row = u32::MAX;
    let mut max_row = 0u32;
    let mut min_col = u32::MAX;
    let mut max_col = 0u32;
    for CellUpdate(id, _, _) in changes {
        min_row = min_row.min(id.row);
        max_row = max_row.max(id.row);
        min_col = min_col.min(id.col);
        max_col = max_col.max(id.col);
    }
    ChangeBounds {
        min_row,
        max_row,
        min_col,
        max_col,
    }
}

// merge adjacent cells into solid rectangular ranges to reduce the number of BFS starting points.
//
// two passes:
// 1. sort row-major, merge consecutive same-row cells into row ranges
// 2. merge consecutive row ranges with the same column span into 2D rectangles
//
// e.g. a 10x10 paste → 10 row ranges → 1 rectangle.
// two disjoint 5x5 blocks → 10 row ranges → 2 rectangles.
fn merge_cells_into_ranges(cells: &[AbsoluteCellId]) -> Vec<CellRange> {
    if cells.len() <= 1 {
        return cells.iter().map(|&c| CellRange::single(c)).collect();
    }

    // pass 1: sort and merge consecutive same-row cells into row ranges
    let mut sorted = cells.to_vec();
    sorted.sort_unstable_by_key(|c| (c.sheet_id, c.row, c.col));

    let mut row_ranges: Vec<CellRange> = Vec::new();
    let (mut start, mut end) = (sorted[0], sorted[0]);
    for &cell in &sorted[1..] {
        if cell.sheet_id == end.sheet_id && cell.row == end.row && cell.col == end.col + 1 {
            end = cell;
        } else {
            row_ranges.push(CellRange::new(
                start.sheet_id,
                start.row,
                start.col,
                end.row,
                end.col,
            ));
            start = cell;
            end = cell;
        }
    }
    row_ranges.push(CellRange::new(
        start.sheet_id,
        start.row,
        start.col,
        end.row,
        end.col,
    ));

    // pass 2: stack consecutive row ranges with the same column span into rectangles
    let mut merged: Vec<CellRange> = Vec::new();
    let mut current = row_ranges[0];
    for &range in &row_ranges[1..] {
        if range.sheet_id == current.sheet_id
            && range.start_row == current.end_row + 1
            && range.start_col == current.start_col
            && range.end_col == current.end_col
        {
            current.end_row = range.end_row;
        } else {
            merged.push(current);
            current = range;
        }
    }
    merged.push(current);
    merged
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    fn cell(row: u32, col: u32) -> AbsoluteCellId {
        AbsoluteCellId {
            sheet_id: 0,
            row,
            col,
        }
    }

    fn assert_cycle_error(engine: &Engine, id: AbsoluteCellId) {
        let value = engine
            .spreadsheet
            .get_cell(&id)
            .map(|cell| cell.val.clone());
        assert_eq!(value, Some(CellValue::err("Cycle")));
    }

    fn assert_cell_number(engine: &Engine, id: AbsoluteCellId, expected: i64) {
        let value = engine
            .spreadsheet
            .get_cell(&id)
            .map(|cell| cell.val.clone());
        assert_eq!(value, Some(CellValue::Number(Decimal::from(expected))));
    }

    #[test]
    fn direct_cycle_sets_cycle_error() {
        let mut engine = Engine::new();
        let guard = engine.start_batch();

        engine.parse_and_insert_string(&guard, cell(0, 0), "=B1");
        engine.parse_and_insert_string(&guard, cell(0, 1), "=A1");
        engine.end_batch(guard);

        assert_cycle_error(&engine, cell(0, 0));
        assert_cycle_error(&engine, cell(0, 1));
    }

    #[test]
    fn formulas_blocked_by_cycle_also_get_cycle_error() {
        let mut engine = Engine::new();
        let guard = engine.start_batch();

        engine.parse_and_insert_string(&guard, cell(0, 0), "=B1");
        engine.parse_and_insert_string(&guard, cell(0, 1), "=A1");
        engine.parse_and_insert_string(&guard, cell(1, 0), "=A1");
        engine.end_batch(guard);

        assert_cycle_error(&engine, cell(0, 0));
        assert_cycle_error(&engine, cell(0, 1));
        assert_cycle_error(&engine, cell(1, 0));
    }

    #[test]
    fn remove_column_removes_selected_column() {
        let mut engine = Engine::new();
        let guard = engine.start_batch();
        engine.insert_value(&guard, cell(0, 0), CellValue::Text("A".into()));
        engine.insert_value(&guard, cell(0, 1), CellValue::Text("B".into()));
        engine.insert_value(&guard, cell(0, 2), CellValue::Text("C".into()));
        engine.end_batch(guard);

        engine.remove_column_or_row(0, false, 1);

        assert!(matches!(
            engine.spreadsheet.get_cell(&cell(0, 0)).map(|c| c.val),
            Some(CellValue::Text(v)) if v == "A"
        ));
        assert!(matches!(
            engine.spreadsheet.get_cell(&cell(0, 1)).map(|c| c.val),
            Some(CellValue::Text(v)) if v == "C"
        ));
        assert!(engine.spreadsheet.get_cell(&cell(0, 2)).is_none());
    }

    #[test]
    fn remove_row_removes_selected_row() {
        let mut engine = Engine::new();
        let guard = engine.start_batch();
        engine.insert_value(&guard, cell(0, 0), CellValue::Text("R0".into()));
        engine.insert_value(&guard, cell(1, 0), CellValue::Text("R1".into()));
        engine.insert_value(&guard, cell(2, 0), CellValue::Text("R2".into()));
        engine.end_batch(guard);

        engine.remove_column_or_row(0, true, 1);

        assert!(matches!(
            engine.spreadsheet.get_cell(&cell(0, 0)).map(|c| c.val),
            Some(CellValue::Text(v)) if v == "R0"
        ));
        assert!(matches!(
            engine.spreadsheet.get_cell(&cell(1, 0)).map(|c| c.val),
            Some(CellValue::Text(v)) if v == "R2"
        ));
        assert!(engine.spreadsheet.get_cell(&cell(2, 0)).is_none());
    }

    #[test]
    fn undo_redo_insert_row_reverts_and_reapplies_structure() {
        let mut engine = Engine::new();
        let guard = engine.start_batch();
        engine.insert_value(&guard, cell(0, 0), CellValue::Text("R0".into()));
        engine.insert_value(&guard, cell(1, 0), CellValue::Text("R1".into()));
        engine.end_batch(guard);

        engine.insert_column_or_row(0, true, 1, true);
        assert!(engine.spreadsheet.get_cell(&cell(1, 0)).is_none());
        assert!(matches!(
            engine.spreadsheet.get_cell(&cell(2, 0)).map(|c| c.val),
            Some(CellValue::Text(v)) if v == "R1"
        ));

        engine.undo();
        assert!(matches!(
            engine.spreadsheet.get_cell(&cell(1, 0)).map(|c| c.val),
            Some(CellValue::Text(v)) if v == "R1"
        ));
        assert!(engine.spreadsheet.get_cell(&cell(2, 0)).is_none());

        engine.redo();
        assert!(engine.spreadsheet.get_cell(&cell(1, 0)).is_none());
        assert!(matches!(
            engine.spreadsheet.get_cell(&cell(2, 0)).map(|c| c.val),
            Some(CellValue::Text(v)) if v == "R1"
        ));
    }

    #[test]
    fn remove_last_column_rewrites_deleted_reference_to_error_atom() {
        let mut engine = Engine::new();
        let guard = engine.start_batch();
        engine.parse_and_insert_string(&guard, cell(5, 6), "5");
        engine.parse_and_insert_string(&guard, cell(5, 5), "=G6+5");
        engine.end_batch(guard);

        let old_formula_id = engine
            .spreadsheet
            .get_cell(&cell(5, 5))
            .and_then(|c| c.defined_by_formula)
            .expect("formula in F6");
        let old_len = engine
            .spreadsheet
            .formulas
            .get(old_formula_id)
            .expect("formula to exist")
            .ast
            .len();

        engine.remove_column_or_row(0, false, 6);

        let formula_cell = engine
            .spreadsheet
            .get_cell(&cell(5, 5))
            .expect("F6 cell to exist");
        assert_eq!(formula_cell.val, CellValue::err("#REF!"));

        let new_formula_id = formula_cell.defined_by_formula.expect("formula id");
        let new_formula = engine
            .spreadsheet
            .formulas
            .get(new_formula_id)
            .expect("formula to exist");
        assert_eq!(new_formula.ast.len(), old_len);
        assert!(new_formula
            .ast
            .iter()
            .any(|expr| matches!(expr, Expr::Atom(ExprAtom::InvalidReferenceError(msg)) if msg == "#REF!")));

        assert!(engine.spreadsheet.get_cell(&cell(5, 6)).is_none());
    }

    #[test]
    fn shared_formula_chain_keeps_first_dependency_after_fill() {
        let mut engine = Engine::new();

        let seed_guard = engine.start_batch();
        for row in 3..=8 {
            engine.insert_number(&seed_guard, cell(row, 4), Decimal::from((row - 2) as i64));
        }
        engine.end_batch(seed_guard);

        let fill_guard = engine.start_batch();
        engine.parse_and_insert_string(&fill_guard, cell(3, 5), "=E4+F3");
        let formula_id = engine
            .spreadsheet
            .get_cell(&cell(3, 5))
            .and_then(|cell| cell.defined_by_formula)
            .expect("formula in F4");
        for row in 4..=8 {
            engine.insert_shared_formula(&fill_guard, cell(row, 5), formula_id);
        }
        engine.end_batch(fill_guard);

        let mut e4_dependants = engine
            .spreadsheet
            .dependency_graph
            .direct_dependants_for_range(CellRange::single(cell(3, 4)));
        e4_dependants.sort_unstable_by_key(|id| (id.sheet_id, id.row, id.col));
        assert_eq!(e4_dependants, vec![cell(3, 5)]);

        let delete_guard = engine.start_batch();
        engine.delete(&delete_guard, cell(3, 4));
        engine.end_batch(delete_guard);

        assert_cell_number(&engine, cell(3, 5), 0);
        assert_cell_number(&engine, cell(4, 5), 2);
        assert_cell_number(&engine, cell(5, 5), 5);
        assert_cell_number(&engine, cell(6, 5), 9);
        assert_cell_number(&engine, cell(7, 5), 14);
        assert_cell_number(&engine, cell(8, 5), 20);
    }

    #[test]
    fn undo_after_deleting_first_shared_formula_cell_preserves_remaining_edges() {
        let mut engine = Engine::new();

        let seed_guard = engine.start_batch();
        for row in 3..=8 {
            engine.insert_number(&seed_guard, cell(row, 4), Decimal::from((row - 2) as i64));
        }
        engine.end_batch(seed_guard);

        let fill_guard = engine.start_batch();
        engine.parse_and_insert_string(&fill_guard, cell(3, 5), "=E4+F3");
        let formula_id = engine
            .spreadsheet
            .get_cell(&cell(3, 5))
            .and_then(|cell| cell.defined_by_formula)
            .expect("formula in F4");
        for row in 4..=8 {
            engine.insert_shared_formula(&fill_guard, cell(row, 5), formula_id);
        }
        engine.end_batch(fill_guard);

        let delete_formula_guard = engine.start_batch();
        engine.delete(&delete_formula_guard, cell(3, 5));
        engine.end_batch(delete_formula_guard);
        engine.undo();

        let mut e5_dependants = engine
            .spreadsheet
            .dependency_graph
            .direct_dependants_for_range(CellRange::single(cell(4, 4)));
        e5_dependants.sort_unstable_by_key(|id| (id.sheet_id, id.row, id.col));
        assert_eq!(e5_dependants, vec![cell(4, 5)]);
    }

    #[test]
    #[ignore]
    fn bench_eval_insert_and_recalc() {
        const ROWS: u32 = 400;
        const DATA_COLS: u32 = 12; // A..L
        const EDITS: u32 = 500;

        let mut engine = Engine::new();

        // create dense numeric block + one row sum per row (M) + one global sum (N)
        let guard = engine.start_batch();
        for row in 0..ROWS {
            for col in 0..DATA_COLS {
                engine.insert_number(&guard, cell(row, col), Decimal::from((row + col) as i64));
            }
            engine.parse_and_insert_string(
                &guard,
                cell(row, DATA_COLS),
                &format!("=SUM(A{}:L{})", row + 1, row + 1),
            );
        }
        engine.parse_and_insert_string(
            &guard,
            cell(ROWS, DATA_COLS + 1),
            &format!("=SUM(M1:M{})", ROWS),
        );
        engine.end_batch(guard);

        // warm up to stabilize caches / branch predictors
        for i in 0..50 {
            let row = i % ROWS;
            let g = engine.start_batch();
            engine.insert_number(&g, cell(row, 0), Decimal::from((i * 3) as i64));
            engine.end_batch(g);
        }

        // benchmark steady-state edit + recalculation path
        let timer = Instant::now();
        for i in 0..EDITS {
            let row = i % ROWS;
            let g = engine.start_batch();
            engine.insert_number(&g, cell(row, 0), Decimal::from((i * 7) as i64));
            engine.end_batch(g);
        }
        let elapsed = timer.elapsed();

        eprintln!(
            "bench_eval_insert_and_recalc: edits={}, total={:?}, per_edit={:?}",
            EDITS,
            elapsed,
            elapsed / EDITS
        );
    }

    #[test]
    fn remove_column_no_false_cycles_in_formula_grid() {
        // Reproduce: 21 rows × 12 cols. Col A = numbers, cols B-L = "=prev_col + 5".
        // Removing col H should NOT produce Cycle errors.
        let mut engine = Engine::new();
        let guard = engine.start_batch();

        let rows = 21u32;
        let cols = 12u32; // A(0) .. L(11)

        // col A: numbers 1..=21
        for row in 0..rows {
            engine.insert_number(&guard, cell(row, 0), Decimal::from(row as i64 + 1));
        }

        // cols B-L: each cell = same row, previous column + 5
        for row in 0..rows {
            for col in 1..cols {
                let col_letter = std::char::from_u32('A' as u32 + col - 1).unwrap();
                let formula = format!("={}{}+5", col_letter, row + 1);
                engine.parse_and_insert_string(&guard, cell(row, col), &formula);
            }
        }
        engine.end_batch(guard);

        // verify pre-removal: all cells should have numbers
        for row in 0..rows {
            for col in 0..cols {
                let c = engine
                    .spreadsheet
                    .get_cell(&cell(row, col))
                    .expect("cell exists");
                assert!(
                    matches!(&c.val, CellValue::Number(_)),
                    "pre-removal: cell ({},{}) should be number, got {:?}",
                    row,
                    col,
                    c.val
                );
            }
        }

        // remove column H (col 7)
        engine.remove_column_or_row(0, false, 7);

        // after removal: 11 columns remain. No cell should be Cycle error.
        for row in 0..rows {
            for col in 0..(cols - 1) {
                if let Some(c) = engine.spreadsheet.get_cell(&cell(row, col)) {
                    assert!(
                        !matches!(&c.val, CellValue::Error(s, _) if &*s == "Cycle"),
                        "post-removal: cell ({},{}) has false Cycle error",
                        row,
                        col
                    );
                }
            }
        }
    }
}
