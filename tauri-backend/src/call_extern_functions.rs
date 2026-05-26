use crate::ipc_encoding::{self, ExtFnArg};
use crate::storage::grid::CellValue;

pub use crate::engine::EvalError;

type OneshotTx = futures::channel::oneshot::Sender<CellValue>;

struct ExtCallRequest {
    func_name: String,
    args: Vec<ExtFnArg>,
    resp_tx: OneshotTx,
}

static CALL_TX: std::sync::OnceLock<crossbeam_channel::Sender<ExtCallRequest>> =
    std::sync::OnceLock::new();
static CALL_RX: std::sync::OnceLock<crossbeam_channel::Receiver<ExtCallRequest>> =
    std::sync::OnceLock::new();
static PENDING_SENDERS: parking_lot::Mutex<Vec<OneshotTx>> = parking_lot::Mutex::new(Vec::new());

pub fn init() {
    let (tx, rx) = crossbeam_channel::unbounded();
    CALL_TX.set(tx).ok();
    CALL_RX.set(rx).ok();
}

/// JS polls this. Body = binary CellValues (responses from previous batch).
/// Returns binary ext fn calls (next batch). While JS processes a batch,
/// Rust accumulates the next one from forte workers.
#[tauri::command(async)]
pub fn ext_fn_poll(request: tauri::ipc::Request<'_>) -> tauri::ipc::Response {
    if let tauri::ipc::InvokeBody::Raw(bytes) = request.body() {
        if !bytes.is_empty() {
            let senders = std::mem::take(&mut *PENDING_SENDERS.lock());
            let mut offset = 0;
            let vals: Vec<CellValue> = senders
                .iter()
                .map(|_| ipc_encoding::decode_cell_value(bytes, &mut offset))
                .collect();
            for (tx, val) in senders.into_iter().zip(vals) {
                let _ = tx.send(val);
            }
        }
    }

    let rx = CALL_RX.get().unwrap();
    let mut calls: Vec<(String, Vec<ExtFnArg>)> = Vec::new();
    let mut senders: Vec<OneshotTx> = Vec::new();
    while let Ok(req) = rx.try_recv() {
        calls.push((req.func_name, req.args));
        senders.push(req.resp_tx);
    }

    if calls.is_empty() {
        tauri::ipc::Response::new(Vec::new())
    } else {
        let mut buf = Vec::new();
        ipc_encoding::encode_ext_fn_calls(&mut buf, &calls);
        *PENDING_SENDERS.lock() = senders;
        tauri::ipc::Response::new(buf)
    }
}

pub async fn call(func_name: &str, args: Vec<ExtFnArg>) -> Result<CellValue, EvalError> {
    let (tx, rx) = futures::channel::oneshot::channel();
    CALL_TX
        .get()
        .ok_or_else(|| EvalError::Error("ext-ipc not initialized".into()))?
        .send(ExtCallRequest {
            func_name: func_name.to_string(),
            args,
            resp_tx: tx,
        })
        .map_err(|_| EvalError::Error("ext-ipc channel closed".into()))?;
    rx.await
        .map_err(|_| EvalError::Error(format!("{}(): response dropped", func_name)))
}
