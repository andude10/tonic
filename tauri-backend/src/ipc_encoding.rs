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

/// Encode a batch of ext function calls.
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

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;

    fn roundtrip(val: &CellValue) -> CellValue {
        let mut buf = Vec::new();
        encode_cell_value(&mut buf, val);
        let mut offset = 0;
        decode_cell_value(&buf, &mut offset)
    }

    #[test]
    fn roundtrip_number() {
        let val = CellValue::Number(Decimal::new(314, 2));
        assert_eq!(roundtrip(&val), val);
    }

    #[test]
    fn roundtrip_text() {
        let val = CellValue::Text("hello".into());
        assert_eq!(roundtrip(&val), val);
    }

    #[test]
    fn roundtrip_bool() {
        assert_eq!(roundtrip(&CellValue::Bool(true)), CellValue::Bool(true));
        assert_eq!(roundtrip(&CellValue::Bool(false)), CellValue::Bool(false));
    }

    #[test]
    fn roundtrip_error() {
        let val = CellValue::err("bad");
        let decoded = roundtrip(&val);
        assert!(matches!(decoded, CellValue::Error(s, _) if s.as_str() == "bad"));
    }

    #[test]
    fn decode_empty_tag() {
        let mut buf = Vec::new();
        encode_empty(&mut buf);
        let mut offset = 0;
        let val = decode_cell_value(&buf, &mut offset);
        assert_eq!(val, CellValue::Text("".into()));
    }

    #[test]
    fn multiple_values_in_sequence() {
        let mut buf = Vec::new();
        let vals = [
            CellValue::Number(Decimal::from(1)),
            CellValue::Text("x".into()),
            CellValue::Bool(true),
        ];
        for v in &vals {
            encode_cell_value(&mut buf, v);
        }
        let mut offset = 0;
        for expected in &vals {
            assert_eq!(&decode_cell_value(&buf, &mut offset), expected);
        }
    }

    #[test]
    fn encode_ext_fn_calls_structure() {
        let calls = vec![(
            "double".to_string(),
            vec![ExtFnArg::Value(CellValue::Number(Decimal::from(42)))],
        )];
        let mut buf = Vec::new();
        encode_ext_fn_calls(&mut buf, &calls);
        let mut o = 0;
        let name_len = u32::from_le_bytes(buf[o..o + 4].try_into().unwrap()) as usize;
        o += 4;
        assert_eq!(&buf[o..o + name_len], b"double");
        o += name_len;
        let arg_count = u32::from_le_bytes(buf[o..o + 4].try_into().unwrap());
        o += 4;
        assert_eq!(arg_count, 1);
        let decoded = decode_cell_value(&buf, &mut o);
        assert_eq!(decoded, CellValue::Number(Decimal::from(42)));
    }
}
