use crate::storage::{
    cell_properties::CellProperties,
    grid::{Cell, CellValue, GridCellId},
};
use rust_decimal::Decimal;

const TAG_EMPTY: u8 = 0;
const TAG_TEXT: u8 = 1;
const TAG_NUMBER: u8 = 2;
const TAG_BOOL: u8 = 3;
const TAG_ERROR: u8 = 4;
const TAG_RANGE: u8 = 5;

// todo: review the whole file

fn write_bytes(buf: &mut Vec<u8>, bytes: &[u8]) {
    buf.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    buf.extend_from_slice(bytes);
}

fn write_len_prefixed(buf: &mut Vec<u8>, tag: u8, s: &[u8]) {
    buf.push(tag);
    write_bytes(buf, s);
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

// --- Viewport cell encoding (used by get_display_cells) ---

/// Format: [row: u32 LE][col: u32 LE][flags: u8][display_len: u32 LE][display_bytes]
///   if error flag set: [error_msg_len: u32 LE][error_msg_bytes]
///   [format_flags: u8]
///   if text color flag set: [text_color_len: u32 LE][text_color_bytes]
/// Flags: bit 0 = formula, bit 1 = pending, bit 2 = error
/// Format flags: bit 0 = bold, bit 1 = italic, bit 2 = strikethrough, bit 3 = text color
pub fn encode_viewport_cell(
    buf: &mut Vec<u8>,
    row: u32,
    col: u32,
    cell: Option<&Cell>,
    properties: &CellProperties,
    properties_id: GridCellId,
    pending: bool,
) {
    let cell = cell.filter(|cell| {
        // empty error is only a formula placeholder during recalculation.
        pending || !matches!(&cell.val, CellValue::Error(s, _) if s.is_empty())
    });
    let visible_cell = if pending { None } else { cell };
    let mut flags = u8::from(cell.is_some_and(|cell| cell.defined_by_formula.is_some()));
    // pending cells keep formula metadata but hide stale display/error text.
    if pending {
        flags |= 2;
    }
    let error_msg = if let Some(CellValue::Error(msg, _)) = visible_cell.map(|cell| &cell.val) {
        flags |= 4;
        Some(msg.as_str())
    } else {
        None
    };
    let display = visible_cell
        .map(|cell| cell.val.to_string())
        .unwrap_or_default();

    buf.extend_from_slice(&row.to_le_bytes());
    buf.extend_from_slice(&col.to_le_bytes());
    buf.push(flags);
    write_bytes(buf, display.as_bytes());
    // error bytes are present only when the error flag is set.
    if let Some(msg) = error_msg {
        write_bytes(buf, msg.as_bytes());
    }
    encode_cell_properties(buf, properties, properties_id);
}

fn encode_cell_properties(buf: &mut Vec<u8>, properties: &CellProperties, cell: GridCellId) {
    let color = properties.get_cell_color(cell);
    let flags = u8::from(properties.cell_bold(cell))
        | (u8::from(properties.cell_italic(cell)) << 1)
        | (u8::from(properties.cell_strikethrough(cell)) << 2)
        | (u8::from(color.is_some()) << 3);
    buf.push(flags);
    // color bytes are present only when the color flag is set.
    if let Some(color) = color {
        write_bytes(buf, color.as_bytes());
    }
}

// --- ext_fn_poll encoding ---

/// Argument to an external JS function: either a single CellValue or a 2D range.
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
