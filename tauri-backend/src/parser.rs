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
use fastnum::D256;

use std::collections::HashMap;

use crate::sheet::{
    CellId, CellRange, Dependency, Expr, ExprAtom, ExprId, NameRef, SheetId, UserFuncId,
};

// --- Tokens ---

#[derive(Debug, Clone, PartialEq)]
pub enum Token<'src> {
    Number(D256),
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
            .map(|s: &str| Token::Number(s.parse::<D256>().unwrap())),
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
    pub cell_id: CellId,
    pub sheet_names: &'a mut HashMap<String, SheetId>,
    pub cell_names: &'a mut HashMap<NameRef, CellId>,
    pub user_function_names: &'a mut HashMap<NameRef, UserFuncId>,
    pub dependencies: &'a mut HashMap<CellId, Vec<Dependency>>,
    pub dependents: &'a mut HashMap<CellId, Vec<CellId>>,
    pub expr_arena: Vec<Expr>,
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

fn add_dependency(
    cell_id: CellId,
    dep: Dependency,
    dependencies: &mut HashMap<CellId, Vec<Dependency>>,
    dependents: &mut HashMap<CellId, Vec<CellId>>,
) {
    // for each cell in dependency, add currently parsed cell (cell_id) into list of dependants
    dep.for_each_cell(|ref_cell| {
        dependents.entry(ref_cell).or_default().push(cell_id);
    });
    // add dependency to dependencies of currently parsed cell
    dependencies.entry(cell_id).or_default().push(dep);
}

fn remove_dependency(
    cell_id: CellId,
    dep: &Dependency,
    dependents: &mut HashMap<CellId, Vec<CellId>>,
) {
    dep.for_each_cell(|ref_cell| {
        if let Some(entries) = dependents.get_mut(&ref_cell) {
            entries.retain(|c| *c != cell_id);
        }
    });
}

fn push_expr(state: &mut FormulaState, expr: Expr) -> ExprId {
    // if expression (expr) is a reference, then add it to list of
    // dependencies of currently parsed cell (cell_id)
    match &expr {
        Expr::Atom(ExprAtom::CellRef(s, c)) => {
            add_dependency(
                state.cell_id,
                Dependency::SingleCell(*s, *c),
                state.dependencies,
                state.dependents,
            );
        }
        Expr::Atom(ExprAtom::CellRange(s, r)) => {
            add_dependency(
                state.cell_id,
                Dependency::Range(*s, *r),
                state.dependencies,
                state.dependents,
            );
        }
        _ => {}
    }

    // push expression into arena
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
                        let &sheet_id = st.sheet_names.get(first).ok_or_else(|| {
                            Rich::custom(span, format!("unknown sheet '{first}'"))
                        })?;
                        Ok((sheet_id, name))
                    }
                    None => Ok((0 as SheetId, first)),
                }
            });

        // cell_id == A1-style token, produces CellId (defaults to sheet 0)
        let cell_id = select_ref! { Token::Cell(col, row) => CellId { col: *col, row: *row } };

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
                            Expr::Sum {
                                range_id: args[0],
                                sum: D256::ZERO,
                            }
                        }
                        "avg" => {
                            if args.len() != 1 {
                                return Err(Rich::custom(span, "avg expects exactly 1 argument"));
                            }
                            Expr::Avg {
                                range_id: args[0],
                                sum: D256::ZERO,
                                count: 0,
                            }
                        }
                        _ => {
                            let func_id = st
                                .user_function_names
                                .get(&NameRef {
                                    sheet_id,
                                    name: name.into(),
                                })
                                .copied()
                                .ok_or_else(|| {
                                    Rich::custom(span, format!("unresolved function '{name}'"))
                                })?;
                            Expr::ExtrnalFunctionCall { func_id, args }
                        }
                    }
                } else {
                    // cell ref: bare name
                    let cell_id = st
                        .cell_names
                        .get(&NameRef {
                            sheet_id,
                            name: name.into(),
                        })
                        .copied()
                        .ok_or_else(|| Rich::custom(span, format!("unresolved cell '{name}'")))?;
                    Expr::Atom(ExprAtom::CellRef(sheet_id, cell_id))
                };
                Ok(push_expr(extra.state(), expr))
            },
        );

        // A1 or A1:B2 == produces ExprAtom::CellRef or ExprAtom::CellRange
        let cell_or_range = cell_id
            .clone()
            .then(just(Token::Colon).ignore_then(cell_id.clone()).or_not())
            .map(|(start, end)| match end {
                Some(end) => ExprAtom::CellRange(0, CellRange { start, end }),
                None => ExprAtom::CellRef(0, start),
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
    // clear old dependencies before parsing (push_expr will populate new ones)
    let cell_id = state.cell_id;
    if let Some(old_deps) = state.dependencies.get_mut(&cell_id) {
        for dep in old_deps.iter() {
            remove_dependency(cell_id, dep, state.dependents);
        }
        old_deps.clear();
    }

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

/// Convert 0-indexed column to letter(s): 0→A, 25→Z, 26→AA
fn col_to_letters(buf: &mut String, mut col: u32) {
    let mut tmp = [0u8; 4];
    let mut len = 0;
    loop {
        tmp[len] = b'A' + (col % 26) as u8;
        len += 1;
        if col < 26 {
            break;
        }
        col = col / 26 - 1;
    }
    for i in (0..len).rev() {
        buf.push(tmp[i] as char);
    }
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

/// Offset all cell references (A1, A1:B2) in a formula string.
/// Returns a new string with adjusted references, or the original if nothing changed.
pub fn offset_refs(formula: &str, row_off: i32, col_off: i32) -> String {
    if row_off == 0 && col_off == 0 {
        return formula.to_string();
    }

    let bytes = formula.as_bytes();
    let mut result = String::with_capacity(formula.len());
    let mut pos = 0;

    while pos < bytes.len() {
        // todo: make escape logic less hidden
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
            if let Some((col, row, end)) = parse_cell_at(bytes, pos) {
                let new_col = (col as i32 + col_off).max(0) as u32;
                let new_row = (row as i32 + row_off).max(0) as u32;
                col_to_letters(&mut result, new_col);
                result.push_str(&(new_row + 1).to_string());

                // check for range: A1:B2
                if end < bytes.len() && bytes[end] == b':' {
                    if let Some((col2, row2, end2)) = parse_cell_at(bytes, end + 1) {
                        let new_col2 = (col2 as i32 + col_off).max(0) as u32;
                        let new_row2 = (row2 as i32 + row_off).max(0) as u32;
                        result.push(':');
                        col_to_letters(&mut result, new_col2);
                        result.push_str(&(new_row2 + 1).to_string());
                        pos = end2;
                        continue;
                    }
                }
                pos = end;
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
    use crate::sheet::Spreadsheet;
    use fastnum::dec256;

    /// Lex + parse a formula string, return (arena, root_id) or panic with errors.
    fn parse(src: &str, spreadsheet: &mut Spreadsheet) -> (Vec<Expr>, ExprId) {
        let tokens = lex_formula(src).into_output().expect("lexer failed");
        let sheet = &mut spreadsheet.sheets[0];
        let mut state = FormulaState {
            cell_id: CellId { col: 0, row: 0 },
            sheet_names: &mut spreadsheet.sheet_names,
            cell_names: &mut spreadsheet.cell_names,
            user_function_names: &mut spreadsheet.user_function_names,
            dependencies: &mut sheet.dependencies,
            dependents: &mut sheet.dependents,
            expr_arena: Vec::new(),
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
        let mut ss = Spreadsheet::new();
        for (src, expected_arena) in cases {
            let (arena, _root) = parse(src, &mut ss);
            assert_eq!(arena, *expected_arena, "failed for input: {src:?}");
        }
    }

    fn c(col: u32, row: u32) -> CellId {
        CellId { col, row }
    }

    #[test]
    fn test_number_literal() {
        assert_parses(&[
            ("42", vec![Expr::Atom(ExprAtom::Number(dec256!(42)))]),
            ("3.14", vec![Expr::Atom(ExprAtom::Number(dec256!(3.14)))]),
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
            ("A1", vec![Expr::Atom(ExprAtom::CellRef(0, c(0, 0)))]),
            ("B3", vec![Expr::Atom(ExprAtom::CellRef(0, c(1, 2)))]),
        ]);
    }

    #[test]
    fn test_cell_range() {
        assert_parses(&[(
            "A1:B2",
            vec![Expr::Atom(ExprAtom::CellRange(
                0,
                CellRange {
                    start: c(0, 0),
                    end: c(1, 1),
                },
            ))],
        )]);
    }

    #[test]
    fn test_addition() {
        assert_parses(&[(
            "1 + 2",
            vec![
                Expr::Atom(ExprAtom::Number(dec256!(1))),
                Expr::Atom(ExprAtom::Number(dec256!(2))),
                Expr::Add(0, 1),
            ],
        )]);
    }

    #[test]
    fn test_subtraction() {
        assert_parses(&[(
            "5 - 3",
            vec![
                Expr::Atom(ExprAtom::Number(dec256!(5))),
                Expr::Atom(ExprAtom::Number(dec256!(3))),
                Expr::Subtract(0, 1),
            ],
        )]);
    }

    #[test]
    fn test_multiplication() {
        assert_parses(&[(
            "2 * 3",
            vec![
                Expr::Atom(ExprAtom::Number(dec256!(2))),
                Expr::Atom(ExprAtom::Number(dec256!(3))),
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
                Expr::Atom(ExprAtom::Number(dec256!(1))),
                Expr::Atom(ExprAtom::Number(dec256!(2))),
                Expr::Atom(ExprAtom::Number(dec256!(3))),
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
                Expr::Atom(ExprAtom::Number(dec256!(1))),
                Expr::Atom(ExprAtom::Number(dec256!(2))),
                Expr::Add(0, 1),
                Expr::Atom(ExprAtom::Number(dec256!(3))),
                Expr::Multiply(2, 3),
            ],
        )]);
    }

    #[test]
    fn test_negation() {
        assert_parses(&[(
            "-1",
            vec![Expr::Atom(ExprAtom::Number(dec256!(1))), Expr::Negate(0)],
        )]);
    }

    #[test]
    fn test_builtin_sum() {
        let mut ss = Spreadsheet::new();
        let (arena, root) = parse("sum(A1:B2)", &mut ss);
        assert_eq!(
            arena,
            vec![
                Expr::Atom(ExprAtom::CellRange(
                    0,
                    CellRange {
                        start: c(0, 0),
                        end: c(1, 1),
                    }
                )),
                Expr::Sum {
                    range_id: 0,
                    sum: D256::ZERO
                }
            ]
        );
        assert_eq!(root, 1);
    }

    #[test]
    fn test_builtin_avg() {
        let mut ss = Spreadsheet::new();
        let (arena, root) = parse("avg(A1:B2)", &mut ss);
        assert_eq!(
            arena,
            vec![
                Expr::Atom(ExprAtom::CellRange(
                    0,
                    CellRange {
                        start: c(0, 0),
                        end: c(1, 1),
                    }
                )),
                Expr::Avg {
                    range_id: 0,
                    sum: D256::ZERO,
                    count: 0
                }
            ]
        );
        assert_eq!(root, 1);
    }

    #[test]
    fn test_complex_expression() {
        assert_parses(&[(
            "A1 + 2 * 3",
            vec![
                Expr::Atom(ExprAtom::CellRef(0, c(0, 0))),
                Expr::Atom(ExprAtom::Number(dec256!(2))),
                Expr::Atom(ExprAtom::Number(dec256!(3))),
                Expr::Multiply(1, 2),
                Expr::Add(0, 3),
            ],
        )]);
    }

    #[test]
    fn test_named_cell_ref() {
        let mut ss = Spreadsheet::new();
        let cell = CellId { col: 5, row: 10 };
        ss.cell_names.insert(
            NameRef {
                sheet_id: 0,
                name: "total".into(),
            },
            cell,
        );
        let (arena, _) = parse("total", &mut ss);
        assert_eq!(arena, vec![Expr::Atom(ExprAtom::CellRef(0, cell))]);
    }

    #[test]
    fn test_user_function_call() {
        let mut ss = Spreadsheet::new();
        ss.user_function_names.insert(
            NameRef {
                sheet_id: 0,
                name: "myfunc".into(),
            },
            0,
        );
        ss.user_functions.push(Default::default());
        let (arena, root) = parse("myfunc(1, 2)", &mut ss);
        assert_eq!(
            arena,
            vec![
                Expr::Atom(ExprAtom::Number(dec256!(1))),
                Expr::Atom(ExprAtom::Number(dec256!(2))),
                Expr::ExtrnalFunctionCall {
                    func_id: 0,
                    args: vec![0, 1]
                },
            ]
        );
        assert_eq!(root, 2);
    }
}
