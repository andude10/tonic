use std::fmt;

use chumsky::{
    extra,
    input::{Input as _, MapExtra, MappedInput},
    inspector::SimpleState,
    prelude::*,
    recursive::Direct,
    text,
};

use crate::sheet::{CellId, CellRange, Expr, ExprId, ExprValue, OptSheetId};

// --- Tokens ---

#[derive(Debug, Clone, PartialEq)]
pub enum Token<'src> {
    Number(f64),
    True,
    False,
    Name(&'src str),
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
}

impl fmt::Display for Token<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Token::Number(x) => write!(f, "{x}"),
            Token::True => write!(f, "true"),
            Token::False => write!(f, "false"),
            Token::Name(s) => write!(f, "{s}"),
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
        }
    }
}

type Spanned<T> = chumsky::span::Spanned<T, SimpleSpan>;

// --- Error utilities ---

pub fn parse_errors_to_string<'a>(errs: impl IntoIterator<Item = &'a Rich<'a, char>>) -> String {
    errs.into_iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("; ")
}

pub fn token_errors_to_string<'a, 'src: 'a>(
    errs: impl IntoIterator<Item = &'a Rich<'a, Token<'src>>>,
) -> String {
    errs.into_iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("; ")
}

/// Create lexer, which has:
/// Input: &'src str
/// Output: Vec<Token<'src>>
/// Error: extra::Err<Rich<'src, char>>
pub fn create_lexer<'src>(
) -> impl Parser<'src, &'src str, Vec<Spanned<Token<'src>>>, extra::Err<Rich<'src, char>>> {
    choice((
        text::ident().map(|s| match s {
            "true" => Token::True,
            "false" => Token::False,
            s => Token::Name(s),
        }),
        just('+').to(Token::Plus),
        just('-').to(Token::Dash),
        just('*').to(Token::Star),
        just('/').to(Token::Slash),
        just('.').to(Token::Dot),
        just('~').to(Token::Tilde),
        just(':').to(Token::Colon),
        just(',').to(Token::Comma),
        just('(').to(Token::LParen),
        just(')').to(Token::RParen),
        // note: "Number" is deliberately after "Dot", to handle decimal numbers like 2.15
        text::int(10)
            .then(just('.').then(text::digits(10)).or_not())
            .to_slice()
            .map(|s: &str| Token::Number(s.parse().unwrap())),
    ))
    .spanned()
    .padded()
    .repeated()
    .collect()
}

// --- Cell ref helpers ---

/// Try to parse a name like "A1", "BC23" into a CellId.
/// Letters → col (A=0, B=1, ..., Z=25, AA=26, ...), digits → row (1-based → 0-based).
fn parse_cell_name(name: &str) -> Option<CellId> {
    let first_digit = name.find(|c: char| c.is_ascii_digit())?;
    if first_digit == 0 {
        return None;
    }
    let (letters, digits) = name.split_at(first_digit);
    if !letters.bytes().all(|b| b.is_ascii_alphabetic()) {
        return None;
    }
    let row: u32 = digits.parse().ok()?;
    if row == 0 {
        return None;
    }
    let col = letters.bytes().fold(0u32, |acc, b| {
        acc * 26 + (b.to_ascii_uppercase() - b'A') as u32 + 1
    }) - 1;
    Some(CellId { col, row: row - 1 })
}

// --- Formula parser ---

type ExprArena = Vec<Expr>;

fn push_expr(state: &mut SimpleState<ExprArena>, expr: Expr) -> ExprId {
    let id = state.len() as ExprId;
    state.push(expr);
    id
}

type FormulaExtra<'tokens, 'src> =
    extra::Full<Rich<'tokens, Token<'src>>, SimpleState<ExprArena>, ()>;

pub fn create_formula_parser<'tokens, 'src: 'tokens>() -> impl Parser<
    'tokens,
    MappedInput<'tokens, Token<'src>, SimpleSpan, &'tokens [Spanned<Token<'src>>]>,
    ExprId,
    FormulaExtra<'tokens, 'src>,
> {
    recursive(
        |expr: Recursive<
            Direct<
                'tokens,
                '_,
                MappedInput<'tokens, Token<'src>, SimpleSpan, &'tokens [Spanned<Token<'src>>]>,
                ExprId,
                FormulaExtra<'tokens, 'src>,
            >,
        >| {
            // --- atoms ---

            let number = select! { Token::Number(x) => x }
                .map_with(|x, e| push_expr(e.state(), Expr::Literal(ExprValue::Number(x))));

            let boolean = select! {
                Token::True => true,
                Token::False => false,
            }
            .map_with(|b, e| push_expr(e.state(), Expr::Literal(ExprValue::Boolean(b))));

            // cell ref: a Name that matches the cell pattern (e.g. A1, BC23)
            let cell_ref = select! { Token::Name(name) => name }.try_map(|name, span| {
                parse_cell_name(name).ok_or_else(|| {
                    Rich::custom(span, format!("expected cell reference, got '{name}'"))
                })
            });

            // cell_ref optionally followed by ':' cell_ref → CellRange or single CellRef
            let cell_range_or_ref = cell_ref
                .clone()
                .then(just(Token::Colon).ignore_then(cell_ref.clone()).or_not())
                .map_with(|(start, end), e| match end {
                    Some(end) => push_expr(
                        e.state(),
                        Expr::Literal(ExprValue::CellRange(
                            OptSheetId::None,
                            CellRange { start, end },
                        )),
                    ),
                    None => push_expr(
                        e.state(),
                        Expr::Literal(ExprValue::CellRef(OptSheetId::None, start)),
                    ),
                });

            // relative: '~' (cell ref or range)
            let relative = just(Token::Tilde)
                .ignore_then(
                    cell_ref
                        .clone()
                        .then(just(Token::Colon).ignore_then(cell_ref).or_not()),
                )
                .map_with(|(start, end), e| match end {
                    Some(end) => push_expr(
                        e.state(),
                        Expr::Literal(ExprValue::RelativeCellRange(
                            OptSheetId::None,
                            CellRange { start, end },
                        )),
                    ),
                    None => push_expr(
                        e.state(),
                        Expr::Literal(ExprValue::RelativeCellRef(OptSheetId::None, start)),
                    ),
                });

            // function call: SUM(...) or AVG(...)
            let func_call = select! { Token::Name(name) => name }
                .then(
                    expr.clone()
                        .separated_by(just(Token::Comma))
                        .collect::<Vec<ExprId>>()
                        .delimited_by(just(Token::LParen), just(Token::RParen)),
                )
                .try_map_with(
                    |(name, args): (&str, Vec<ExprId>),
                     e: &mut MapExtra<'_, '_, _, FormulaExtra<'_, '_>>| {
                        let span = e.span();
                        match name.to_ascii_uppercase().as_str() {
                            "SUM" => {
                                if args.len() != 1 {
                                    return Err(Rich::custom(
                                        span,
                                        "SUM expects exactly 1 argument",
                                    ));
                                }
                                Ok(push_expr(e.state(), Expr::Sum(args[0])))
                            }
                            "AVG" => {
                                if args.len() != 1 {
                                    return Err(Rich::custom(
                                        span,
                                        "AVG expects exactly 1 argument",
                                    ));
                                }
                                Ok(push_expr(e.state(), Expr::Avg(args[0])))
                            }
                            _ => Err(Rich::custom(span, format!("unknown function '{name}'"))),
                        }
                    },
                );

            let atom = choice((func_call, relative, cell_range_or_ref, number, boolean));

            // --- arithmetic with precedence ---

            let term = atom.clone().foldl_with(
                choice((just(Token::Star).to(true), just(Token::Slash).to(false)))
                    .then(atom)
                    .repeated(),
                |lhs, (is_mul, rhs), e: &mut MapExtra<'_, '_, _, FormulaExtra<'_, '_>>| {
                    if is_mul {
                        push_expr(e.state(), Expr::Multiply(lhs, rhs))
                    } else {
                        push_expr(e.state(), Expr::Divide(lhs, rhs))
                    }
                },
            );

            term.clone().foldl_with(
                choice((just(Token::Plus).to(true), just(Token::Dash).to(false)))
                    .then(term)
                    .repeated(),
                |lhs, (is_add, rhs), e: &mut MapExtra<'_, '_, _, FormulaExtra<'_, '_>>| {
                    if is_add {
                        push_expr(e.state(), Expr::Add(lhs, rhs))
                    } else {
                        push_expr(e.state(), Expr::Subtract(lhs, rhs))
                    }
                },
            )
        },
    )
}

/// Parse a formula (already lexed) into a flat Vec<Expr> arena.
/// Returns the arena and the root ExprId, or errors.
pub fn parse_formula<'tokens, 'src: 'tokens>(
    tokens: &'tokens [Spanned<Token<'src>>],
    eoi: SimpleSpan,
) -> (Option<(Vec<Expr>, ExprId)>, Vec<Rich<'tokens, Token<'src>>>) {
    let mut arena = SimpleState(Vec::new());
    let result = create_formula_parser().parse_with_state(tokens.split_spanned(eoi), &mut arena);
    let errs: Vec<_> = result.errors().cloned().collect();
    let output = result.into_output().map(|root| (arena.0, root));
    (output, errs)
}
