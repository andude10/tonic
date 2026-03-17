use std::{
    fmt,
    sync::{Arc, LazyLock},
};

use chumsky::{
    cache::{Cache, Cached},
    extra,
    input::{self, Cursor, Input as _, MappedInput},
    inspector::Inspector,
    pratt::{infix, left, prefix},
    prelude::*,
    text,
};
use rust_decimal::Decimal;

use crate::storage::{
    grid::GridCellId,
    name_resolution::SpreadsheetNames,
    types::{AbsoluteCellId, Expr, ExprAtom, ExprId, Reference, SheetId},
};

// --- Tokens ---

#[derive(Debug, Clone, PartialEq)]
pub enum Token<'src> {
    Number(Decimal),
    Text(&'src str),
    True,
    False,
    Name(&'src str),
    Cell(u32, u32),
    Plus,
    Dash,
    Star,
    Slash,
    Dot,
    Colon,
    Comma,
    LParen,
    RParen,
    Backslash,
    Arrow,
}

impl fmt::Display for Token<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Token::Number(x) => write!(f, "{x}"),
            Token::True => write!(f, "true"),
            Token::False => write!(f, "false"),
            Token::Name(s) => write!(f, "{s}"),
            Token::Cell(col, row) => write!(f, "{col}:{row}"),
            Token::Plus => write!(f, "+"),
            Token::Dash => write!(f, "-"),
            Token::Star => write!(f, "*"),
            Token::Slash => write!(f, "/"),
            Token::Dot => write!(f, "."),
            Token::Colon => write!(f, ":"),
            Token::Comma => write!(f, ","),
            Token::LParen => write!(f, "("),
            Token::RParen => write!(f, ")"),
            Token::Backslash => write!(f, "\\"),
            Token::Arrow => write!(f, "->"),
            Token::Text(s) => write!(f, "{s}"),
        }
    }
}

type Spanned<T> = chumsky::span::Spanned<T, SimpleSpan>;

// --- Error utilities ---

pub fn lexer_errors_to_string<'a>(errs: impl IntoIterator<Item = &'a Rich<'a, char>>) -> String {
    errs.into_iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("; ")
}

pub fn parse_formula_errors_to_string<'a, 'src: 'a>(
    errs: impl IntoIterator<Item = &'a Rich<'a, Token<'src>>>,
) -> String {
    errs.into_iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("; ")
}

/// Input string
type LexerInput<'src> = &'src str;
/// Output array of spanned tokens (tokens with character numbers, will be used when generating errors)
type LexerOutput<'src> = Vec<Spanned<Token<'src>>>;
/// "Rich" error type
type LexerExtra<'src> = extra::Err<Rich<'src, char>>;

pub fn create_lexer<'src>(
) -> impl Parser<'src, LexerInput<'src>, LexerOutput<'src>, LexerExtra<'src>> {
    choice((
        // Cell (e.g. A1, BC23)
        chumsky::regex::regex("[a-zA-Z]+[0-9]+").map(|s: &str| {
            let (mut col, mut row) = (0u32, 0u32);
            for b in s.bytes() {
                if b.is_ascii_alphabetic() {
                    col = col * 26 + (b.to_ascii_uppercase() - b'A') as u32 + 1;
                } else {
                    row = row * 10 + (b - b'0') as u32;
                }
            }
            Token::Cell(col - 1, row - 1)
        }),
        text::ident().map(|s| match s {
            "true" => Token::True,
            "false" => Token::False,
            s => Token::Name(s),
        }),
        just('+').to(Token::Plus),
        just('\\').to(Token::Backslash),
        just("->").to(Token::Arrow),
        just('-').to(Token::Dash),
        just('*').to(Token::Star),
        just('/').to(Token::Slash),
        just('.').to(Token::Dot),
        just(':').to(Token::Colon),
        just(',').to(Token::Comma),
        just('(').to(Token::LParen),
        just(')').to(Token::RParen),
        // Text
        just('"')
            .ignore_then(none_of('"').repeated().to_slice())
            .then_ignore(just('"'))
            .map(Token::Text),
        // Number (note: "Number" is deliberately after "Dot", to handle decimal numbers like 2.15)
        text::int(10)
            .then(just('.').then(text::digits(10)).or_not())
            .to_slice()
            .map(|s: &str| Token::Number(s.parse::<Decimal>().unwrap())),
    ))
    .spanned()
    .padded()
    .repeated()
    .collect()
}

// --- Cached lexer ---

#[derive(Default)]
struct CachedLexer;

impl Cached for CachedLexer {
    type Parser<'src> = Arc<
        dyn Parser<'src, LexerInput<'src>, LexerOutput<'src>, LexerExtra<'src>>
            + Send
            + Sync
            + 'src,
    >;

    fn make_parser<'src>(self) -> Self::Parser<'src> {
        Arc::new(create_lexer())
    }
}

static LEXER: LazyLock<Cache<CachedLexer>> = LazyLock::new(Cache::default);

pub fn lex_formula(input: &str) -> ParseResult<LexerOutput<'_>, Rich<'_, char>> {
    LEXER.get().parse(input)
}

/// Input tokens
type FormulaInput<'tokens, 'src> = MappedInput<
    'tokens,
    Token<'src>,                     // token type
    SimpleSpan,                      // span type
    &'tokens [Spanned<Token<'src>>], // actual input type (combined)
>;
/// Id of expression in AST (stored in FormulaState)
type FormulaOutput = ExprId;
/// Expression arena (flat AST)
pub struct FormulaState<'a> {
    pub names: &'a mut SpreadsheetNames,
    pub cell_id: GridCellId,
    pub expr_arena: Vec<Expr>,
    pub dependencies: Vec<AbsoluteCellId>,
}

type FormulaExtra<'tokens, 'src> =
    extra::Full<Rich<'tokens, Token<'src>>, FormulaState<'tokens>, ()>;

// todo: remove this, needed to resolve some big type error
impl<'src, I: Input<'src>> Inspector<'src, I> for FormulaState<'_> {
    type Checkpoint = ();
    fn on_token(&mut self, _: &I::Token) {}
    fn on_save<'parse>(&self, _: &Cursor<'src, 'parse, I>) -> Self::Checkpoint {}
    fn on_rewind<'parse>(&mut self, _: &input::Checkpoint<'src, 'parse, I, Self::Checkpoint>) {}
}

fn push_expr(state: &mut FormulaState, expr: Expr) -> ExprId {
    let id = state.expr_arena.len() as ExprId;
    state.expr_arena.push(expr);
    id
}

/// Parser for formula language. Resolves all references (named cells, functions, sheets)
fn create_formula_praser<'tokens, 'src: 'tokens>(
) -> impl Parser<'tokens, FormulaInput<'tokens, 'src>, FormulaOutput, FormulaExtra<'tokens, 'src>>
       + 'tokens {
    // define recursive parser (use 'expr' to reference to itself)
    recursive(|expr| {
        let ident = select_ref! { Token::Name(s) => *s };

        // qualified_name == Parses sheetName.name or just name, produces (SheetId, &str)
        let qualified_name = ident
            .then(just(Token::Dot).ignore_then(ident).or_not())
            .try_map_with(|(first, second), extra| {
                let span = extra.span();
                let st: &mut FormulaState = extra.state();
                match second {
                    Some(name) => {
                        let &sheet_id = st.names.sheet_names.get(first).ok_or_else(|| {
                            Rich::custom(span, format!("unknown sheet '{first}'"))
                        })?;
                        Ok((sheet_id, name))
                    }
                    None => Ok((0 as SheetId, first)),
                }
            });

        // cell_id == A1-style token, produces GridCellId (defaults to sheet 0)
        let cell_id = select_ref! { Token::Cell(col, row) => GridCellId { col: *col, row: *row } };

        // call_args: (expr, expr, ...)
        let call_args = expr
            .clone()
            .separated_by(just(Token::Comma))
            .collect::<Vec<ExprId>>()
            .delimited_by(just(Token::LParen), just(Token::RParen));

        // cell_name_or_function_call == qualified_name followed by optional (...) for function call, otherwise cell ref
        let cell_name_or_function_call = qualified_name.then(call_args.or_not()).try_map_with(
            |((sheet_id, name), args), extra| {
                let span = extra.span();
                let st: &mut FormulaState = extra.state();
                let expr = if let Some(args) = args {
                    // function call: name(...)
                    match name {
                        "sum" => {
                            if args.len() != 1 {
                                return Err(Rich::custom(span, "sum expects exactly 1 argument"));
                            }
                            Expr::Sum
                        }
                        "avg" => {
                            if args.len() != 1 {
                                return Err(Rich::custom(span, "avg expects exactly 1 argument"));
                            }
                            Expr::Avg
                        }
                        _ => {
                            // todo: user functions not yet supported
                            return Err(Rich::custom(
                                span,
                                format!("unresolved function '{name}'"),
                            ));
                        }
                    }
                } else {
                    // cell ref: bare name
                    let named_cell =
                        st.names.cell_names.get(name).ok_or_else(|| {
                            Rich::custom(span, format!("unresolved cell '{name}'"))
                        })?;
                    let row_offset = named_cell.row as i32 - st.cell_id.row as i32;
                    let col_offset = named_cell.col as i32 - st.cell_id.col as i32;
                    st.dependencies.push(AbsoluteCellId {
                        sheet_id: named_cell.sheet_id,
                        row: named_cell.row,
                        col: named_cell.col,
                    });
                    Expr::Atom(ExprAtom::Reference(Reference::Single {
                        sheet_id: named_cell.sheet_id,
                        row_offset,
                        col_offset,
                    }))
                };
                Ok(push_expr(extra.state(), expr))
            },
        );

        // A1 or A1:B2 == produces Reference with offsets
        let cell_or_range = cell_id
            .clone()
            .then(just(Token::Colon).ignore_then(cell_id.clone()).or_not())
            .map_with(|(start, end), extra| {
                let st: &mut FormulaState = extra.state();
                let start_row_offset = start.row as i32 - st.cell_id.row as i32;
                let start_col_offset = start.col as i32 - st.cell_id.col as i32;
                match end {
                    Some(end) => {
                        let end_row_offset = end.row as i32 - st.cell_id.row as i32;
                        let end_col_offset = end.col as i32 - st.cell_id.col as i32;
                        // add all cells in range as dependencies
                        let min_row = start.row.min(end.row);
                        let max_row = start.row.max(end.row);
                        let min_col = start.col.min(end.col);
                        let max_col = start.col.max(end.col);
                        for row in min_row..=max_row {
                            for col in min_col..=max_col {
                                st.dependencies.push(AbsoluteCellId {
                                    sheet_id: 0,
                                    row,
                                    col,
                                });
                            }
                        }
                        ExprAtom::Reference(Reference::Range {
                            sheet_id: 0,
                            range_start_row_offset: start_row_offset,
                            range_start_col_offset: start_col_offset,
                            range_end_row_offset: end_row_offset,
                            range_end_col_offset: end_col_offset,
                        })
                    }
                    None => {
                        st.dependencies.push(AbsoluteCellId {
                            sheet_id: 0,
                            row: start.row,
                            col: start.col,
                        });
                        ExprAtom::Reference(Reference::Single {
                            sheet_id: 0,
                            row_offset: start_row_offset,
                            col_offset: start_col_offset,
                        })
                    }
                }
            });

        // atom_value == self-contained value (3, false, A1, A1:A5, 3.14, "text", etc)
        let atom_value = choice((
            select_ref! { Token::True => ExprAtom::Boolean(true) },
            select_ref! { Token::False => ExprAtom::Boolean(false) },
            select_ref! { Token::Number(x) => ExprAtom::Number(*x) },
            select_ref! { Token::Text(s) => ExprAtom::Text(s.to_string()) },
            cell_or_range,
        ))
        .map_with(|val, extra| push_expr(extra.state(), Expr::Atom(val)));

        // parenthesized expression: ( expr )
        let paren_expr = expr
            .clone()
            .delimited_by(just(Token::LParen), just(Token::RParen));

        let atom = choice((cell_name_or_function_call, paren_expr, atom_value));

        atom.pratt((
            prefix(3, just(Token::Dash), |_, r, e| {
                push_expr(e.state(), Expr::Negate(r))
            }),
            infix(left(2), just(Token::Star), |l, _, r, e| {
                push_expr(e.state(), Expr::Multiply(l, r))
            }),
            infix(left(2), just(Token::Slash), |l, _, r, e| {
                push_expr(e.state(), Expr::Divide(l, r))
            }),
            infix(left(1), just(Token::Plus), |l, _, r, e| {
                push_expr(e.state(), Expr::Add(l, r))
            }),
            infix(left(1), just(Token::Dash), |l, _, r, e| {
                push_expr(e.state(), Expr::Subtract(l, r))
            }),
        ))
    })
}

/// Parse a formula (already lexed) into a flat Vec<Expr> arena.
/// Returns the arena and the root ExprId, or errors.
pub fn parse_formula<'tokens, 'src: 'tokens>(
    tokens: &'tokens [Spanned<Token<'src>>],
    src_len: usize,
    state: &mut FormulaState<'tokens>,
) -> (Option<(Vec<Expr>, ExprId)>, Vec<Rich<'tokens, Token<'src>>>) {
    let eoi = SimpleSpan::new((), src_len..src_len);
    let result = create_formula_praser()
        .boxed()
        .parse_with_state(tokens.split_spanned(eoi), state);
    let errs: Vec<_> = result.errors().cloned().collect();
    let output = result
        .into_output()
        .map(|root| (std::mem::take(&mut state.expr_arena), root));
    (output, errs)
}

/// Parse a cell reference at position `pos` in `s` (letters then digits).
/// Returns (col 0-indexed, row 0-indexed, end position) or None.
fn parse_cell_at(s: &[u8], pos: usize) -> Option<(u32, u32, usize)> {
    let mut i = pos;
    if i >= s.len() || !s[i].is_ascii_alphabetic() {
        return None;
    }
    let mut col: u32 = 0;
    while i < s.len() && s[i].is_ascii_alphabetic() {
        col = col * 26 + (s[i].to_ascii_uppercase() - b'A') as u32 + 1;
        i += 1;
    }
    col -= 1;
    if i >= s.len() || !s[i].is_ascii_digit() {
        return None;
    }
    let mut row: u32 = 0;
    while i < s.len() && s[i].is_ascii_digit() {
        row = row * 10 + (s[i] - b'0') as u32;
        i += 1;
    }
    row -= 1; // 1-indexed in text -> 0-indexed
    Some((col, row, i))
}

/// Shift all cell references in a formula string using AST references.
/// References in the string and AST appear in the same left-to-right order.
/// `cell_id` is the cell where the formula will be displayed.
/// `ast` contains the parsed expressions with R1C1 offsets.
/// `names` is used to convert absolute cell IDs to names.
pub fn shift_formula_refs(
    formula: &str,
    cell_id: &GridCellId,
    ast: &[Expr],
    names: &SpreadsheetNames,
) -> String {
    use crate::storage::types::{ExprAtom, Reference};

    // collect all references from AST in order
    let mut refs: Vec<&Reference> = Vec::new();
    for expr in ast {
        if let Expr::Atom(ExprAtom::Reference(r)) = expr {
            refs.push(r);
        }
    }

    let bytes = formula.as_bytes();
    let mut result = String::with_capacity(formula.len());
    let mut pos = 0;
    let mut ref_idx = 0;

    while pos < bytes.len() {
        // skip quoted strings
        if bytes[pos] == b'"' {
            result.push('"');
            pos += 1;
            while pos < bytes.len() && bytes[pos] != b'"' {
                result.push(bytes[pos] as char);
                pos += 1;
            }
            if pos < bytes.len() {
                result.push('"');
                pos += 1;
            }
            continue;
        }

        if bytes[pos].is_ascii_alphabetic() {
            if let Some((_col, _row, end)) = parse_cell_at(bytes, pos) {
                // Check for range: A1:B2
                let is_range = end < bytes.len() && bytes[end] == b':';
                let final_end = if is_range {
                    parse_cell_at(bytes, end + 1)
                        .map(|(_, _, e)| e)
                        .unwrap_or(end)
                } else {
                    end
                };

                // Use AST reference if available
                if ref_idx < refs.len() {
                    let r = refs[ref_idx];
                    ref_idx += 1;
                    match r {
                        Reference::Single {
                            sheet_id,
                            row_offset,
                            col_offset,
                        } => {
                            let abs_row = (cell_id.row as i32 + row_offset).max(0) as u32;
                            let abs_col = (cell_id.col as i32 + col_offset).max(0) as u32;
                            let abs_id = AbsoluteCellId {
                                sheet_id: *sheet_id,
                                row: abs_row,
                                col: abs_col,
                            };
                            result.push_str(&names.cell_id_to_name(&abs_id));
                        }
                        Reference::Range {
                            sheet_id,
                            range_start_row_offset,
                            range_start_col_offset,
                            range_end_row_offset,
                            range_end_col_offset,
                        } => {
                            let start_row =
                                (cell_id.row as i32 + range_start_row_offset).max(0) as u32;
                            let start_col =
                                (cell_id.col as i32 + range_start_col_offset).max(0) as u32;
                            let end_row = (cell_id.row as i32 + range_end_row_offset).max(0) as u32;
                            let end_col = (cell_id.col as i32 + range_end_col_offset).max(0) as u32;
                            let start_id = AbsoluteCellId {
                                sheet_id: *sheet_id,
                                row: start_row,
                                col: start_col,
                            };
                            let end_id = AbsoluteCellId {
                                sheet_id: *sheet_id,
                                row: end_row,
                                col: end_col,
                            };
                            result.push_str(&names.cell_id_to_name(&start_id));
                            result.push(':');
                            result.push_str(&names.cell_id_to_name(&end_id));
                        }
                    }
                    pos = final_end;
                    continue;
                }
                // Fallback: copy original reference
                while pos < final_end {
                    result.push(bytes[pos] as char);
                    pos += 1;
                }
                continue;
            }
        }
        result.push(bytes[pos] as char);
        pos += 1;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;
    use std::str::FromStr;

    macro_rules! dec {
        ($val:expr) => {
            Decimal::from_str(stringify!($val)).unwrap()
        };
    }

    /// Lex + parse a formula string, return (arena, root_id) or panic with errors.
    fn parse(src: &str, names: &mut SpreadsheetNames) -> (Vec<Expr>, ExprId) {
        let tokens = lex_formula(src).into_output().expect("lexer failed");
        let mut state = FormulaState {
            names,
            cell_id: GridCellId { col: 0, row: 0 },
            expr_arena: Vec::new(),
            dependencies: Vec::new(),
        };
        let (parsed, errs) = parse_formula(&tokens, src.len(), &mut state);
        assert!(
            errs.is_empty(),
            "parse errors: {}",
            parse_formula_errors_to_string(&errs)
        );
        parsed.expect("no output")
    }

    fn assert_parses(cases: &[(&str, Vec<Expr>)]) {
        let mut names = SpreadsheetNames::new();
        for (src, expected_arena) in cases {
            let (arena, _root) = parse(src, &mut names);
            assert_eq!(arena, *expected_arena, "failed for input: {src:?}");
        }
    }

    /// Helper to create a Reference::Single with offsets (relative to cell 0,0)
    fn r(col: i32, row: i32) -> Reference {
        Reference::Single {
            sheet_id: 0,
            row_offset: row,
            col_offset: col,
        }
    }

    #[test]
    fn test_number_literal() {
        assert_parses(&[
            ("42", vec![Expr::Atom(ExprAtom::Number(dec!(42)))]),
            ("3.14", vec![Expr::Atom(ExprAtom::Number(dec!(3.14)))]),
        ]);
    }

    #[test]
    fn test_boolean_literal() {
        assert_parses(&[
            ("true", vec![Expr::Atom(ExprAtom::Boolean(true))]),
            ("false", vec![Expr::Atom(ExprAtom::Boolean(false))]),
        ]);
    }

    #[test]
    fn test_text_literal() {
        assert_parses(&[(
            r#""hello""#,
            vec![Expr::Atom(ExprAtom::Text("hello".into()))],
        )]);
    }

    #[test]
    fn test_cell_ref() {
        assert_parses(&[
            ("A1", vec![Expr::Atom(ExprAtom::Reference(r(0, 0)))]),
            ("B3", vec![Expr::Atom(ExprAtom::Reference(r(1, 2)))]),
        ]);
    }

    #[test]
    fn test_cell_range() {
        assert_parses(&[(
            "A1:B2",
            vec![Expr::Atom(ExprAtom::Reference(Reference::Range {
                sheet_id: 0,
                range_start_row_offset: 0,
                range_start_col_offset: 0,
                range_end_row_offset: 1,
                range_end_col_offset: 1,
            }))],
        )]);
    }

    #[test]
    fn test_addition() {
        assert_parses(&[(
            "1 + 2",
            vec![
                Expr::Atom(ExprAtom::Number(dec!(1))),
                Expr::Atom(ExprAtom::Number(dec!(2))),
                Expr::Add(0, 1),
            ],
        )]);
    }

    #[test]
    fn test_subtraction() {
        assert_parses(&[(
            "5 - 3",
            vec![
                Expr::Atom(ExprAtom::Number(dec!(5))),
                Expr::Atom(ExprAtom::Number(dec!(3))),
                Expr::Subtract(0, 1),
            ],
        )]);
    }

    #[test]
    fn test_multiplication() {
        assert_parses(&[(
            "2 * 3",
            vec![
                Expr::Atom(ExprAtom::Number(dec!(2))),
                Expr::Atom(ExprAtom::Number(dec!(3))),
                Expr::Multiply(0, 1),
            ],
        )]);
    }

    #[test]
    fn test_precedence() {
        // 1 + 2 * 3 => 1, 2, 3, Mul(1,2), Add(0,3)
        assert_parses(&[(
            "1 + 2 * 3",
            vec![
                Expr::Atom(ExprAtom::Number(dec!(1))),
                Expr::Atom(ExprAtom::Number(dec!(2))),
                Expr::Atom(ExprAtom::Number(dec!(3))),
                Expr::Multiply(1, 2),
                Expr::Add(0, 3),
            ],
        )]);
    }

    #[test]
    fn test_parentheses() {
        // (1 + 2) * 3 => 1, 2, Add(0,1), 3, Mul(2,3)
        assert_parses(&[(
            "(1 + 2) * 3",
            vec![
                Expr::Atom(ExprAtom::Number(dec!(1))),
                Expr::Atom(ExprAtom::Number(dec!(2))),
                Expr::Add(0, 1),
                Expr::Atom(ExprAtom::Number(dec!(3))),
                Expr::Multiply(2, 3),
            ],
        )]);
    }

    #[test]
    fn test_negation() {
        assert_parses(&[(
            "-1",
            vec![Expr::Atom(ExprAtom::Number(dec!(1))), Expr::Negate(0)],
        )]);
    }

    #[test]
    fn test_builtin_sum() {
        let mut names = SpreadsheetNames::new();
        let (arena, root) = parse("sum(A1:B2)", &mut names);
        assert_eq!(
            arena,
            vec![
                Expr::Atom(ExprAtom::Reference(Reference::Range {
                    sheet_id: 0,
                    range_start_row_offset: 0,
                    range_start_col_offset: 0,
                    range_end_row_offset: 1,
                    range_end_col_offset: 1,
                })),
                Expr::Sum
            ]
        );
        assert_eq!(root, 1);
    }

    #[test]
    fn test_builtin_avg() {
        let mut names = SpreadsheetNames::new();
        let (arena, root) = parse("avg(A1:B2)", &mut names);
        assert_eq!(
            arena,
            vec![
                Expr::Atom(ExprAtom::Reference(Reference::Range {
                    sheet_id: 0,
                    range_start_row_offset: 0,
                    range_start_col_offset: 0,
                    range_end_row_offset: 1,
                    range_end_col_offset: 1,
                })),
                Expr::Avg
            ]
        );
        assert_eq!(root, 1);
    }

    #[test]
    fn test_complex_expression() {
        assert_parses(&[(
            "A1 + 2 * 3",
            vec![
                Expr::Atom(ExprAtom::Reference(r(0, 0))),
                Expr::Atom(ExprAtom::Number(dec!(2))),
                Expr::Atom(ExprAtom::Number(dec!(3))),
                Expr::Multiply(1, 2),
                Expr::Add(0, 3),
            ],
        )]);
    }

    #[test]
    fn test_named_cell_ref() {
        let mut names = SpreadsheetNames::new();
        let cell = AbsoluteCellId {
            sheet_id: 0,
            col: 5,
            row: 10,
        };
        names.cell_names.insert("total".into(), cell.clone());
        let (arena, _) = parse("total", &mut names);
        // Offset from (0,0) to (5,10) is (5,10)
        assert_eq!(
            arena,
            vec![Expr::Atom(ExprAtom::Reference(Reference::Single {
                sheet_id: 0,
                row_offset: 10,
                col_offset: 5,
            }))]
        );
    }
}
