use crate::storage::grid::{Cell, CellValue};
use rust_decimal::Decimal;

const TAG_EMPTY: u8 = 0;
const TAG_TEXT: u8 = 1;
const TAG_NUMBER: u8 = 2;
const TAG_BOOL: u8 = 3;
const TAG_ERROR: u8 = 4;
const TAG_RANGE: u8 = 5;

fn write_len_prefixed(buf: &mut Vec<u8>, tag: u8, s: &[u8]) {
    buf.push(tag);
    buf.extend_from_slice(&(s.len() as u32).to_le_bytes());
    buf.extend_from_slice(s);
}

// --- CellValue encode/decode ---

pub fn encode_cell_value(buf: &mut Vec<u8>, val: &CellValue) {
    match val {
        CellValue::Text(s) => write_len_prefixed(buf, TAG_TEXT, s.as_bytes()),
        CellValue::Number(n) => write_len_prefixed(buf, TAG_NUMBER, n.to_string().as_bytes()),
        CellValue::Bool(b) => {
            write_len_prefixed(buf, TAG_BOOL, if *b { b"true" } else { b"false" })
        }
        CellValue::Error(msg, _) => write_len_prefixed(buf, TAG_ERROR, msg.as_bytes()),
    }
}

pub fn encode_empty(buf: &mut Vec<u8>) {
    write_len_prefixed(buf, TAG_EMPTY, &[]);
}

pub fn decode_cell_value(bytes: &[u8], offset: &mut usize) -> CellValue {
    let tag = bytes[*offset];
    *offset += 1;
    let len = u32::from_le_bytes(bytes[*offset..*offset + 4].try_into().unwrap()) as usize;
    *offset += 4;
    let s = std::str::from_utf8(&bytes[*offset..*offset + len]).unwrap_or_default();
    *offset += len;
    match tag {
        TAG_NUMBER => match s.parse::<Decimal>() {
            Ok(n) => CellValue::Number(n),
            Err(_) => CellValue::Text(s.into()),
        },
        TAG_BOOL => CellValue::Bool(s == "true"),
        TAG_ERROR => CellValue::err(s),
        _ => CellValue::Text(s.into()),
    }
}

// --- Viewport cell encoding (used by get_cells_in_viewport) ---

/// Format: [row: u32 LE][col: u32 LE][flags: u8][display_len: u32 LE][display_bytes]
///   if error flag set: [error_msg_len: u32 LE][error_msg_bytes]
/// Flags: bit 0 = formula, bit 1 = pending, bit 2 = error
pub fn encode_viewport_cell(buf: &mut Vec<u8>, row: u32, col: u32, cell: Option<&Cell>) {
    buf.extend_from_slice(&row.to_le_bytes());
    buf.extend_from_slice(&col.to_le_bytes());
    match cell {
        Some(cell) => {
            let mut flags = 0u8;
            if cell.defined_by_formula.is_some() {
                flags |= 1;
            }
            let error_msg = if let CellValue::Error(msg, _) = &cell.val {
                flags |= 4;
                Some(msg.as_str())
            } else {
                None
            };
            buf.push(flags);
            let display = cell.val.to_string();
            let display_bytes = display.as_bytes();
            buf.extend_from_slice(&(display_bytes.len() as u32).to_le_bytes());
            buf.extend_from_slice(display_bytes);
            if let Some(msg) = error_msg {
                let msg_bytes = msg.as_bytes();
                buf.extend_from_slice(&(msg_bytes.len() as u32).to_le_bytes());
                buf.extend_from_slice(msg_bytes);
            }
        }
        None => {
            buf.push(0);
            buf.extend_from_slice(&0u32.to_le_bytes());
        }
    }
}

// --- ext_fn_poll encoding ---

/// Argument to an external JS function — either a single CellValue or a 2D range.
pub enum ExtFnArg {
    Value(CellValue),
    Empty,
    Range {
        rows: usize,
        cols: usize,
        values: Vec<CellValue>,
    },
}

/// Encode a batch of ext function calls into binary.
/// Format per call: [func_name_len: u32][func_name][arg_count: u32][args...]
pub fn encode_ext_fn_calls(buf: &mut Vec<u8>, calls: &[(String, Vec<ExtFnArg>)]) {
    for (func_name, args) in calls {
        let name_bytes = func_name.as_bytes();
        buf.extend_from_slice(&(name_bytes.len() as u32).to_le_bytes());
        buf.extend_from_slice(name_bytes);
        buf.extend_from_slice(&(args.len() as u32).to_le_bytes());
        for arg in args {
            match arg {
                ExtFnArg::Value(v) => encode_cell_value(buf, v),
                ExtFnArg::Empty => encode_empty(buf),
                ExtFnArg::Range { rows, cols, values } => {
                    buf.push(TAG_RANGE);
                    buf.extend_from_slice(&(*rows as u32).to_le_bytes());
                    buf.extend_from_slice(&(*cols as u32).to_le_bytes());
                    for v in values {
                        encode_cell_value(buf, v);
                    }
                }
            }
        }
    }
}
