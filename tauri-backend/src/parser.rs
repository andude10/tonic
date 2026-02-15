use crate::sheet::{CellValue, Function};

// parser parses two kind of languages:
// 1. Actual formula language that user writes
// 2. formula's IR, which is used when saving sheet to file
// 3. function's IR, which is used when saving functions to functions.csv

pub fn parse_formula_textual_representation(text: &str) -> Formula {}

pub fn parse_function_textual_representation(text: &str) -> Function {}
