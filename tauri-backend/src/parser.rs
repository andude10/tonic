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

use crate::sheet::{
    CellId, CellRange, Expr, ExprAtom, ExprId, NameRef, SheetId, Spreadsheet, UserFunction,
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
    Tilde,
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
            Token::Tilde => write!(f, "~"),
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
        just('~').to(Token::Tilde),
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
struct FormulaState<'tokens> {
    spreadsheet: &'tokens mut Spreadsheet,
    expr_arena: Vec<Expr>,
}

type FormulaExtra<'tokens, 'src> =
    extra::Full<Rich<'tokens, Token<'src>>, FormulaState<'tokens>, ()>;

// todo: remove this, needed to resolve some big type error in get_spreadsheet
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

fn get_spreadsheet<'a>(state: &'a mut FormulaState) -> &'a mut Spreadsheet {
    state.spreadsheet
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
                let sp = get_spreadsheet(extra.state());
                match second {
                    Some(name) => {
                        let &sheet_id = sp.sheet_names.get(first).ok_or_else(|| {
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
                let sp = get_spreadsheet(extra.state());
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
                            let func_id = sp
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
                    let cell_id = sp
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

        // ~A1 or ~A1:B2 == produces ExprValue::RelativeCellRef or ExprValue::RelativeCellRange
        let relative = just(Token::Tilde).ignore_then(
            cell_id
                .clone()
                .then(just(Token::Colon).ignore_then(cell_id).or_not())
                .map(|(start, end)| match end {
                    Some(end) => ExprAtom::RelativeCellRange(0, CellRange { start, end }),
                    None => ExprAtom::RelativeCellRef(0, start),
                }),
        );

        // atom_value == self-contained value (3, false, A1, ~A1, A1:A5, 3.14, "text", etc)
        let atom_value = choice((
            select_ref! { Token::True => ExprAtom::Boolean(true) },
            select_ref! { Token::False => ExprAtom::Boolean(false) },
            select_ref! { Token::Number(x) => ExprAtom::Number(*x) },
            select_ref! { Token::Text(s) => ExprAtom::Text(s.to_string()) },
            relative,
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
    spreadsheet: &'tokens mut Spreadsheet,
) -> (Option<(Vec<Expr>, ExprId)>, Vec<Rich<'tokens, Token<'src>>>) {
    let eoi = SimpleSpan::new((), src_len..src_len);
    let mut state = FormulaState {
        spreadsheet,
        expr_arena: Vec::new(),
    };
    let result = create_formula_praser()
        .boxed()
        .parse_with_state(tokens.split_spanned(eoi), &mut state);
    let errs: Vec<_> = result.errors().cloned().collect();
    let output = result.into_output().map(|root| (state.expr_arena, root));
    (output, errs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fastnum::dec256;

    /// Lex + parse a formula string, return (arena, root_id) or panic with errors.
    fn parse(src: &str, spreadsheet: &mut Spreadsheet) -> (Vec<Expr>, ExprId) {
        let tokens = lex_formula(src).into_output().expect("lexer failed");
        let (parsed, errs) = parse_formula(&tokens, src.len(), spreadsheet);
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
    fn test_relative_cell_ref() {
        assert_parses(&[(
            "~A1",
            vec![Expr::Atom(ExprAtom::RelativeCellRef(0, c(0, 0)))],
        )]);
    }

    #[test]
    fn test_relative_cell_range() {
        assert_parses(&[(
            "~A1:B2",
            vec![Expr::Atom(ExprAtom::RelativeCellRange(
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
