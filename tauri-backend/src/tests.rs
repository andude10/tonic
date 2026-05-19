use std::collections::BTreeMap;

use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::json;
use tauri::{
    http::HeaderMap,
    ipc::{InvokeBody, InvokeResponseBody},
    test::{get_ipc_response, mock_builder, mock_context, noop_assets, MockRuntime, INVOKE_KEY},
    WebviewWindow, WebviewWindowBuilder,
};

use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
struct ViewCell {
    row: u32,
    col: u32,
    is_formula: bool,
    display: String,
    error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct FilterOptionSnapshot {
    id: u32,
    val: String,
    selected: bool,
}

struct TestHarness {
    _app: tauri::App<MockRuntime>,
    webview: WebviewWindow<MockRuntime>,
}

impl TestHarness {
    fn new() -> Self {
        // build the same backend wiring the app uses, then drive it through ipc.
        let app = build_app(mock_builder())
            .build(mock_context(noop_assets()))
            .expect("test app to build");
        let mut app = app;
        setup(&mut app).expect("test setup");
        let webview = WebviewWindowBuilder::new(&app, "test", Default::default())
            .build()
            .expect("test webview to build");
        let harness = Self { _app: app, webview };
        harness.invoke_unit("new_file", json!({}));
        harness
    }

    fn invoke_response(
        &self,
        cmd: &str,
        body: serde_json::Value,
        headers: HeaderMap,
    ) -> Result<InvokeResponseBody, serde_json::Value> {
        get_ipc_response(
            &self.webview,
            tauri::webview::InvokeRequest {
                cmd: cmd.into(),
                callback: tauri::ipc::CallbackFn(0),
                error: tauri::ipc::CallbackFn(1),
                url: "tauri://localhost".parse().expect("invoke url"),
                body: InvokeBody::Json(body),
                headers,
                invoke_key: INVOKE_KEY.to_string(),
            },
        )
    }

    fn invoke_ok<T: for<'de> Deserialize<'de>>(&self, cmd: &str, body: serde_json::Value) -> T {
        self.invoke_ok_with_headers(cmd, body, HeaderMap::new())
    }

    fn invoke_ok_with_headers<T: for<'de> Deserialize<'de>>(
        &self,
        cmd: &str,
        body: serde_json::Value,
        headers: HeaderMap,
    ) -> T {
        match self.invoke_response(cmd, body, headers) {
            Ok(body) => body.deserialize().unwrap_or_else(|err| {
                panic!("failed to deserialize '{cmd}' response: {err}");
            }),
            Err(err) => panic!("command '{cmd}' failed: {}", error_string(err)),
        }
    }

    fn invoke_unit(&self, cmd: &str, body: serde_json::Value) {
        self.invoke_ok::<()>(cmd, body);
    }

    fn invoke_raw_ok(&self, cmd: &str, body: serde_json::Value, headers: HeaderMap) -> Vec<u8> {
        match self.invoke_response(cmd, body, headers) {
            Ok(InvokeResponseBody::Raw(bytes)) => bytes,
            Ok(InvokeResponseBody::Json(text)) => {
                panic!("expected raw response from '{cmd}', got json: {text}");
            }
            Err(err) => panic!("command '{cmd}' failed: {}", error_string(err)),
        }
    }

    fn paste(&self, cells: &[(u32, u32, &str)]) {
        let pairs: Vec<(CellId, String)> = cells
            .iter()
            .map(|&(row, col, text)| (cell(row, col), text.to_string()))
            .collect();
        self.invoke_unit("paste_values", json!({ "cells": pairs }));
    }

    fn delete(&self, cells: &[(u32, u32)]) {
        let ids: Vec<CellId> = cells.iter().map(|&(row, col)| cell(row, col)).collect();
        self.invoke_unit("delete_cells", json!({ "cells": ids }));
    }

    fn fill_from_single_source(&self, source_row: u32, source_col: u32, dests: &[(u32, u32)]) {
        let source = cell(source_row, source_col);
        let sources = vec![source; dests.len()];
        let dests: Vec<CellId> = dests.iter().map(|&(row, col)| cell(row, col)).collect();
        self.invoke_unit(
            "fill_cells",
            json!({
                "sources": sources,
                "dests": dests,
                "origMinRow": source_row,
                "origMaxRow": source_row,
                "origMinCol": source_col,
                "origMaxCol": source_col,
            }),
        );
    }

    fn editor_value(&self, row: u32, col: u32) -> String {
        let bytes = self.invoke_raw_ok(
            "get_editor_value_for_cell",
            json!({ "cellId": cell(row, col) }),
            HeaderMap::new(),
        );
        String::from_utf8(bytes).expect("editor value to be utf-8")
    }

    fn viewport_map(
        &self,
        row_start: u32,
        row_end: u32,
        col_start: u32,
        col_end: u32,
    ) -> BTreeMap<(u32, u32), ViewCell> {
        let mut headers = HeaderMap::new();
        headers.insert(
            "row-start",
            row_start.to_string().parse().expect("row-start"),
        );
        headers.insert("row-end", row_end.to_string().parse().expect("row-end"));
        headers.insert(
            "col-start",
            col_start.to_string().parse().expect("col-start"),
        );
        headers.insert("col-end", col_end.to_string().parse().expect("col-end"));

        let bytes = self.invoke_raw_ok("get_display_cells", json!({}), headers);
        decode_viewport(bytes)
            .into_iter()
            .map(|cell| ((cell.row, cell.col), cell))
            .collect()
    }

    fn filter_option_map(
        &self,
        header_row: u32,
        header_col: u32,
    ) -> BTreeMap<String, FilterOptionSnapshot> {
        self.invoke_ok::<Vec<FilterOptionSnapshot>>(
            "get_filter_options_for_table_column",
            json!({ "header": grid_cell(header_row, header_col) }),
        )
        .into_iter()
        .map(|option| (option.val.clone(), option))
        .collect()
    }
}

fn error_string(value: serde_json::Value) -> String {
    if let Ok(text) = serde_json::from_value::<String>(value.clone()) {
        return text;
    }
    if let Some(message) = value.get("message").and_then(serde_json::Value::as_str) {
        return message.to_string();
    }
    value.to_string()
}

fn cell(row: u32, col: u32) -> CellId {
    CellId { row, col }
}

fn grid_cell(row: u32, col: u32) -> GridCellId {
    GridCellId { row, col }
}

fn read_u32(bytes: &[u8], cursor: &mut usize) -> u32 {
    let start = *cursor;
    let end = start + 4;
    *cursor = end;
    u32::from_le_bytes(bytes[start..end].try_into().expect("u32"))
}

fn read_string(bytes: &[u8], cursor: &mut usize) -> String {
    let len = read_u32(bytes, cursor) as usize;
    let start = *cursor;
    let end = start + len;
    *cursor = end;
    String::from_utf8(bytes[start..end].to_vec()).expect("string")
}

// todo: this is copied from frontend. Test frotnend instead

fn decode_viewport(bytes: Vec<u8>) -> Vec<ViewCell> {
    // decode the same binary cell payload the frontend reads.
    let mut cursor = 0;
    let mut cells = Vec::new();

    while cursor < bytes.len() {
        let row = read_u32(&bytes, &mut cursor);
        let col = read_u32(&bytes, &mut cursor);
        let flags = bytes[cursor];
        cursor += 1;

        let display = read_string(&bytes, &mut cursor);
        let error = if flags & 4 != 0 {
            Some(read_string(&bytes, &mut cursor))
        } else {
            None
        };

        cells.push(ViewCell {
            row,
            col,
            is_formula: flags & 1 != 0,
            display,
            error,
        });
    }

    cells
}

fn extrapolate_for_test(
    row_patterns: &[Option<(Decimal, Decimal)>],
    col_patterns: &[Option<(Decimal, Decimal)>],
    source_row_index: usize,
    source_col_index: usize,
    row_from_origin: i64,
    col_from_origin: i64,
    row_from_source: i64,
    col_from_source: i64,
) -> Option<Decimal> {
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

    let horizontal_value = pattern_value(row_patterns, source_row_index, col_from_origin);
    let vertical_value = pattern_value(col_patterns, source_col_index, row_from_origin);

    if row_from_source != 0 && col_from_source != 0 {
        if let (Some(value), Some(vertical_step)) =
            (horizontal_value, step_value(col_patterns, col_from_origin))
        {
            return Some(value + vertical_step * Decimal::from(row_from_source));
        }

        if let (Some(value), Some(horizontal_step)) =
            (vertical_value, step_value(row_patterns, row_from_origin))
        {
            return Some(value + horizontal_step * Decimal::from(col_from_source));
        }
    }

    if row_from_source != 0 {
        return vertical_value;
    }

    if col_from_source != 0 {
        return horizontal_value;
    }

    None
}

#[test]
fn two_dimensional_fill_combines_row_and_column_progressions() {
    let row_patterns = vec![
        Some((Decimal::from(1), Decimal::from(1))),
        Some((Decimal::from(2), Decimal::from(2))),
    ];
    let col_patterns = vec![
        Some((Decimal::from(1), Decimal::from(1))),
        Some((Decimal::from(2), Decimal::from(2))),
    ];

    let value = extrapolate_for_test(&row_patterns, &col_patterns, 1, 1, 2, 2, 1, 1);

    assert_eq!(value, Some(Decimal::from(9)));
}

#[test]
fn structural_commands_shift_formulas_through_row_and_column_changes() {
    let app = TestHarness::new();
    app.paste(&[
        (0, 0, "10"),
        (0, 1, "20"),
        (0, 2, "=A1+B1"),
        (0, 3, "=C1*2"),
        (1, 0, "5"),
        (1, 1, "=A1+A2"),
    ]);

    let initial = app.viewport_map(0, 1, 0, 3);
    assert_eq!(initial[&(0, 2)].display, "30");
    assert_eq!(initial[&(0, 3)].display, "60");
    assert_eq!(app.editor_value(0, 2), "=A1+B1");
    assert_eq!(app.editor_value(1, 1), "=A1+A2");

    app.invoke_unit("insert_column", json!({ "col": 1, "left": true }));

    let after_insert_column = app.viewport_map(0, 1, 0, 4);
    assert_eq!(after_insert_column[&(0, 0)].display, "10");
    assert_eq!(after_insert_column[&(0, 1)].display, "");
    assert_eq!(after_insert_column[&(0, 2)].display, "20");
    assert_eq!(after_insert_column[&(0, 3)].display, "30");
    assert_eq!(after_insert_column[&(0, 4)].display, "60");
    assert_eq!(app.editor_value(0, 3), "=A1+C1");
    assert_eq!(app.editor_value(0, 4), "=D1*2");

    app.invoke_unit("remove_column", json!({ "col": 1 }));

    let after_remove_column = app.viewport_map(0, 1, 0, 3);
    assert_eq!(after_remove_column[&(0, 2)].display, "30");
    assert_eq!(after_remove_column[&(0, 3)].display, "60");
    assert_eq!(app.editor_value(0, 2), "=A1+B1");
    assert_eq!(app.editor_value(0, 3), "=C1*2");

    app.invoke_unit("insert_row", json!({ "row": 0, "below": true }));

    let after_insert_row = app.viewport_map(0, 2, 0, 1);
    assert_eq!(after_insert_row[&(1, 0)].display, "");
    assert_eq!(after_insert_row[&(2, 0)].display, "5");
    assert_eq!(after_insert_row[&(2, 1)].display, "15");
    assert_eq!(app.editor_value(2, 1), "=A1+A3");

    app.invoke_unit("remove_row", json!({ "row": 1 }));

    let after_remove_row = app.viewport_map(0, 1, 0, 1);
    assert_eq!(after_remove_row[&(1, 0)].display, "5");
    assert_eq!(after_remove_row[&(1, 1)].display, "15");
    assert_eq!(app.editor_value(1, 1), "=A1+A2");
}

#[test]
fn table_projection_commands_filter_sort_and_apply_projection() {
    let app = TestHarness::new();
    app.paste(&[
        (0, 0, "Name"),
        (0, 1, "Status"),
        (0, 2, "Score"),
        (1, 0, "Ada"),
        (1, 1, "keep"),
        (1, 2, "10"),
        (2, 0, "Bob"),
        (2, 1, "drop"),
        (2, 2, "30"),
        (3, 0, "Cara"),
        (3, 1, "keep"),
        (3, 2, "20"),
        (4, 0, "Dana"),
        (4, 1, ""),
        (4, 2, "40"),
    ]);

    let _: u32 = app.invoke_ok(
        "create_table",
        json!({
            "tableName": "people",
            "firstHeader": grid_cell(0, 0),
            "lastHeader": grid_cell(0, 2),
            "bodyStart": grid_cell(1, 0),
            "bodyEnd": grid_cell(4, 2),
        }),
    );

    let initial_filters = app.filter_option_map(0, 1);
    assert!(initial_filters["(Blanks)"].selected);
    assert!(initial_filters["keep"].selected);
    assert!(initial_filters["drop"].selected);

    let drop_id = initial_filters["drop"].id;
    app.invoke_unit(
        "toggle_table_filter",
        json!({ "header": grid_cell(0, 1), "filterOptionId": drop_id }),
    );
    app.invoke_unit(
        "toggle_table_filter",
        json!({ "header": grid_cell(0, 1), "filterOptionId": 0 }),
    );
    app.invoke_unit(
        "toggle_table_sort",
        json!({ "header": grid_cell(0, 2), "desc": true }),
    );

    let filtered = app.filter_option_map(0, 1);
    assert!(!filtered["(Blanks)"].selected);
    assert!(filtered["keep"].selected);
    assert!(!filtered["drop"].selected);

    let projected = app.viewport_map(1, 4, 0, 2);
    assert_eq!(projected[&(1, 0)].display, "Cara");
    assert_eq!(projected[&(1, 1)].display, "keep");
    assert_eq!(projected[&(1, 2)].display, "20");
    assert_eq!(projected[&(2, 0)].display, "Ada");
    assert_eq!(projected[&(2, 1)].display, "keep");
    assert_eq!(projected[&(2, 2)].display, "10");

    app.invoke_unit("apply_table_projection", json!({ "tableName": "people" }));

    let applied = app.viewport_map(0, 4, 0, 2);
    assert_eq!(applied[&(1, 0)].display, "Cara");
    assert_eq!(applied[&(1, 1)].display, "keep");
    assert_eq!(applied[&(1, 2)].display, "20");
    assert_eq!(applied[&(2, 0)].display, "Ada");
    assert_eq!(applied[&(2, 1)].display, "keep");
    assert_eq!(applied[&(2, 2)].display, "10");
    assert_eq!(applied[&(3, 0)].display, "");
    assert_eq!(applied[&(3, 1)].display, "");
    assert_eq!(applied[&(3, 2)].display, "");
    assert_eq!(applied[&(4, 0)].display, "");
    assert_eq!(applied[&(4, 1)].display, "");
    assert_eq!(applied[&(4, 2)].display, "");

    let after_apply = app.filter_option_map(0, 1);
    assert!(after_apply["(Blanks)"].selected);
    assert!(after_apply["keep"].selected);
    assert!(!after_apply.contains_key("drop"));
}

#[test]
fn table_column_formula_refs_parse_and_rename() {
    let app = TestHarness::new();
    app.paste(&[
        (0, 0, "Name"),
        (0, 1, "Qty"),
        (0, 2, "Double"),
        (1, 0, "Ada"),
        (1, 1, "2"),
        (2, 0, "Bob"),
        (2, 1, "4"),
    ]);

    let _: u32 = app.invoke_ok(
        "create_table",
        json!({
            "tableName": "Table1",
            "firstHeader": grid_cell(0, 0),
            "lastHeader": grid_cell(0, 2),
            "bodyStart": grid_cell(1, 0),
            "bodyEnd": grid_cell(2, 2),
        }),
    );

    app.invoke_unit(
        "enter_input",
        json!({ "cellId": cell(1, 2), "userInput": "=B2*2" }),
    );
    app.invoke_unit(
        "enter_input",
        json!({ "cellId": cell(2, 2), "userInput": "=sum(Qty)" }),
    );
    app.invoke_unit(
        "enter_input",
        json!({ "cellId": cell(3, 0), "userInput": "=sum(Table1.Qty)" }),
    );
    app.invoke_unit(
        "enter_input",
        json!({ "cellId": cell(3, 1), "userInput": "=sum(B2:B3)" }),
    );

    let before = app.viewport_map(1, 3, 0, 2);
    assert_eq!(before[&(1, 2)].display, "4");
    assert_eq!(before[&(2, 2)].display, "6");
    assert_eq!(before[&(3, 0)].display, "6");
    assert_eq!(before[&(3, 1)].display, "6");

    app.invoke_unit(
        "enter_input",
        json!({ "cellId": cell(0, 1), "userInput": "Amount" }),
    );

    assert_eq!(app.editor_value(1, 2), "=@Amount*2");
    assert_eq!(app.editor_value(2, 2), "=sum(Amount)");
    assert_eq!(app.editor_value(3, 0), "=sum(Table1.Amount)");
    assert_eq!(app.editor_value(3, 1), "=sum(Table1.Amount)");
}

#[test]
fn dependants_recalculate_through_shared_formula_fill_delete_and_paste() {
    let app = TestHarness::new();
    app.paste(&[
        (3, 4, "1"),
        (4, 4, "2"),
        (5, 4, "3"),
        (6, 4, "4"),
        (7, 4, "5"),
        (8, 4, "6"),
    ]);

    app.invoke_unit(
        "enter_input",
        json!({ "cellId": cell(3, 5), "userInput": "=E4+F3" }),
    );
    app.fill_from_single_source(3, 5, &[(4, 5), (5, 5), (6, 5), (7, 5), (8, 5)]);
    app.invoke_unit(
        "enter_input",
        json!({ "cellId": cell(3, 6), "userInput": "=sum(F4:F9)" }),
    );
    app.invoke_unit(
        "enter_input",
        json!({ "cellId": cell(3, 7), "userInput": "=sum(E4:E9)+G4" }),
    );

    let initial = app.viewport_map(3, 8, 4, 7);
    assert_eq!(initial[&(3, 5)].display, "1");
    assert_eq!(initial[&(4, 5)].display, "3");
    assert_eq!(initial[&(5, 5)].display, "6");
    assert_eq!(initial[&(6, 5)].display, "10");
    assert_eq!(initial[&(7, 5)].display, "15");
    assert_eq!(initial[&(8, 5)].display, "21");
    assert_eq!(initial[&(3, 6)].display, "56");
    assert_eq!(initial[&(3, 7)].display, "77");
    assert_eq!(app.editor_value(8, 5), "=E9+F8");

    app.delete(&[(3, 4)]);

    let after_delete = app.viewport_map(3, 8, 4, 7);
    assert_eq!(after_delete[&(3, 5)].display, "0");
    assert_eq!(after_delete[&(4, 5)].display, "2");
    assert_eq!(after_delete[&(5, 5)].display, "5");
    assert_eq!(after_delete[&(6, 5)].display, "9");
    assert_eq!(after_delete[&(7, 5)].display, "14");
    assert_eq!(after_delete[&(8, 5)].display, "20");
    assert_eq!(after_delete[&(3, 6)].display, "50");
    assert_eq!(after_delete[&(3, 7)].display, "70");

    app.paste(&[(3, 4, "10"), (5, 4, "7")]);

    let after_paste = app.viewport_map(3, 8, 4, 7);
    assert_eq!(after_paste[&(3, 5)].display, "10");
    assert_eq!(after_paste[&(4, 5)].display, "12");
    assert_eq!(after_paste[&(5, 5)].display, "19");
    assert_eq!(after_paste[&(6, 5)].display, "23");
    assert_eq!(after_paste[&(7, 5)].display, "28");
    assert_eq!(after_paste[&(8, 5)].display, "34");
    assert_eq!(after_paste[&(3, 6)].display, "126");
    assert_eq!(after_paste[&(3, 7)].display, "160");
}

#[test]
fn cycle_errors_propagate_to_blocked_dependants_and_recover_after_breaking_cycle() {
    let app = TestHarness::new();
    app.paste(&[(0, 0, "=B1"), (0, 1, "=C1"), (0, 2, "=A1"), (0, 3, "=C1+1")]);

    let with_cycle = app.viewport_map(0, 0, 0, 3);
    assert_eq!(with_cycle[&(0, 0)].error.as_deref(), Some("Cycle"));
    assert_eq!(with_cycle[&(0, 1)].error.as_deref(), Some("Cycle"));
    assert_eq!(with_cycle[&(0, 2)].error.as_deref(), Some("Cycle"));
    assert_eq!(with_cycle[&(0, 3)].error.as_deref(), Some("Cycle"));
    assert_eq!(app.editor_value(0, 0), "=B1");
    assert_eq!(app.editor_value(0, 1), "=C1");
    assert_eq!(app.editor_value(0, 2), "=A1");
    assert_eq!(app.editor_value(0, 3), "=C1+1");

    app.invoke_unit(
        "enter_input",
        json!({ "cellId": cell(0, 2), "userInput": "7" }),
    );

    let recovered = app.viewport_map(0, 0, 0, 3);
    assert_eq!(recovered[&(0, 0)].display, "7");
    assert_eq!(recovered[&(0, 1)].display, "7");
    assert_eq!(recovered[&(0, 2)].display, "7");
    assert_eq!(recovered[&(0, 3)].display, "8");
    assert_eq!(recovered[&(0, 0)].error, None);
    assert_eq!(recovered[&(0, 1)].error, None);
    assert_eq!(recovered[&(0, 2)].error, None);
    assert_eq!(recovered[&(0, 3)].error, None);
}

#[test]
fn undo_redo_replays_structural_and_value_changes_with_correct_recalculation() {
    let app = TestHarness::new();
    app.paste(&[(0, 0, "1"), (1, 0, "2"), (0, 1, "=sum(A1:A2)")]);

    let initial = app.viewport_map(0, 1, 0, 1);
    assert_eq!(initial[&(0, 1)].display, "3");
    assert_eq!(app.editor_value(0, 1), "=sum(A1:A2)");

    app.invoke_unit("insert_row", json!({ "row": 0, "below": true }));

    let after_insert = app.viewport_map(0, 2, 0, 1);
    assert_eq!(after_insert[&(2, 0)].display, "2");
    assert_eq!(after_insert[&(0, 1)].display, "3");
    assert_eq!(app.editor_value(0, 1), "=sum(A1:A3)");

    app.delete(&[(2, 0)]);

    let after_delete = app.viewport_map(0, 2, 0, 1);
    assert_eq!(after_delete[&(2, 0)].display, "");
    assert_eq!(after_delete[&(0, 1)].display, "1");

    let undo_delete = app
        .invoke_ok::<Option<ChangeBounds>>("undo_input", json!({}))
        .expect("delete undo bounds");
    assert_eq!(
        (
            undo_delete.min_row,
            undo_delete.max_row,
            undo_delete.min_col,
            undo_delete.max_col,
        ),
        (2, 2, 0, 0)
    );

    let after_undo_delete = app.viewport_map(0, 2, 0, 1);
    assert_eq!(after_undo_delete[&(2, 0)].display, "2");
    assert_eq!(after_undo_delete[&(0, 1)].display, "3");

    let undo_insert = app.invoke_ok::<Option<ChangeBounds>>("undo_input", json!({}));
    assert!(undo_insert.is_some());

    let after_undo_insert = app.viewport_map(0, 1, 0, 1);
    assert_eq!(after_undo_insert[&(1, 0)].display, "2");
    assert_eq!(after_undo_insert[&(0, 1)].display, "3");
    assert_eq!(app.editor_value(0, 1), "=sum(A1:A2)");

    let redo_insert = app.invoke_ok::<Option<ChangeBounds>>("redo_input", json!({}));
    assert!(redo_insert.is_some());

    let after_redo_insert = app.viewport_map(0, 2, 0, 1);
    assert_eq!(after_redo_insert[&(2, 0)].display, "2");
    assert_eq!(after_redo_insert[&(0, 1)].display, "3");
    assert_eq!(app.editor_value(0, 1), "=sum(A1:A3)");

    let redo_delete = app.invoke_ok::<Option<ChangeBounds>>("redo_input", json!({}));
    assert!(redo_delete.is_some());

    let after_redo_delete = app.viewport_map(0, 2, 0, 1);
    assert_eq!(after_redo_delete[&(2, 0)].display, "");
    assert_eq!(after_redo_delete[&(0, 1)].display, "1");
}

#[test]
fn fuzz_test() {
    use rand::Rng;

    let app = TestHarness::new();
    let grid_size = 20u32;
    let mut rng = rand::rng();
    macro_rules! rc {
        () => {
            (
                rng.random_range(0..grid_size),
                rng.random_range(0..grid_size),
            )
        };
    }

    let formulas = [
        "=A1",
        "=A1+B1",
        "=sum(A1:C3)",
        "=A1*2+1",
        "=if(A1>0,A1,-A1)",
        "=B2",
        "=sum(A1:A20)",
        "=A1+A2+A3",
        "=0",
        "=1/0",
    ];
    let values = ["0", "1", "-1", "999999", "hello", "", "3.14", "true", "=A1"];

    for _ in 0..500 {
        match rng.random_range(0..10u32) {
            0..=2 => {
                let (r, c) = rc!();
                app.paste(&[(r, c, values[rng.random_range(0..values.len())])]);
            }
            3..=4 => {
                let (r, c) = rc!();
                let _ = app.invoke_response(
                    "enter_input",
                    json!({ "cellId": cell(r, c), "userInput": formulas[rng.random_range(0..formulas.len())] }),
                    HeaderMap::new(),
                );
            }
            5 => {
                let cells: Vec<_> = (0..rng.random_range(1..6u32)).map(|_| rc!()).collect();
                app.delete(&cells);
            }
            6 => {
                let (sr, sc) = rc!();
                let dests: Vec<_> = (0..rng.random_range(1..6u32)).map(|_| rc!()).collect();
                app.fill_from_single_source(sr, sc, &dests);
            }
            7 => {
                let _ = app.invoke_response("undo_input", json!({}), HeaderMap::new());
            }
            8 => {
                let _ = app.invoke_response("redo_input", json!({}), HeaderMap::new());
            }
            9 => {
                let _ = app.viewport_map(0, grid_size - 1, 0, grid_size - 1);
            }
            _ => {}
        }
    }

    let view = app.viewport_map(0, grid_size - 1, 0, grid_size - 1);
    assert!(!view.is_empty());

    for _ in 0..10 {
        let pos = rng.random_range(0..grid_size);
        let _ = app.invoke_response(
            "insert_row",
            json!({ "position": pos, "sheetId": 0 }),
            HeaderMap::new(),
        );
        let _ = app.invoke_response(
            "insert_column",
            json!({ "position": pos, "sheetId": 0 }),
            HeaderMap::new(),
        );
    }
    for _ in 0..5 {
        let pos = rng.random_range(0..grid_size);
        let _ = app.invoke_response(
            "remove_row",
            json!({ "position": pos, "sheetId": 0 }),
            HeaderMap::new(),
        );
        let _ = app.invoke_response(
            "remove_column",
            json!({ "position": pos, "sheetId": 0 }),
            HeaderMap::new(),
        );
    }

    let final_view = app.viewport_map(0, grid_size - 1, 0, grid_size - 1);
    assert!(!final_view.is_empty());

    for _ in 0..50 {
        let _ = app.invoke_response("undo_input", json!({}), HeaderMap::new());
    }
    for _ in 0..50 {
        let _ = app.invoke_response("redo_input", json!({}), HeaderMap::new());
    }

    let after_undo_redo = app.viewport_map(0, grid_size - 1, 0, grid_size - 1);
    assert!(!after_undo_redo.is_empty());
}
