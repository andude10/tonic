use crate::sheet::{Formula, UserFunction};

pub fn parse_formula_textual_representation(text: &str) -> Formula {
    // This function is not implemented. The new file format uses serde + RON
    // and does not require a manual IR parser.
    // This stub remains for parsing the user-facing formula language in the future.
    unimplemented!(
        "Formula parsing from textual IR is obsolete. Text: {}",
        text
    )
}

pub fn parse_function_textual_representation(text: &str) -> UserFunction {
    // This function is not implemented. The new file format uses serde + RON
    // and does not require a manual IR parser.
    // This stub remains for parsing the user-facing formula language in the future.
    unimplemented!(
        "UserFunction parsing from textual IR is obsolete. Text: {}",
        text
    )
}
