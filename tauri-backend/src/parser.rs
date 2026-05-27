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
    types::{
        AbsoluteCellId, CellRange, Coordinate, Expr, ExprAtom, ExprId, Formula, FormulaTemplateRef,
        Reference, TableId,
    },
};

// --- Tokens ---

#[derive(Debug, Clone, PartialEq)]
pub enum Token<'src> {
    Number(Decimal),
    Text(&'src str),
    True,
    False,
    Name(&'src str),
    Cell {
        col: u32,
        row: u32,
        abs_col: bool,
        abs_row: bool,
    },
    Plus,
    Dash,
    Star,
    Slash,
    Dot,
    At,
    Colon,
    Comma,
    LParen,
    RParen,
    Eq,
    Gt,
    Lt,
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
            Token::Cell {
                col,
                row,
                abs_col,
                abs_row,
            } => {
                if *abs_col {
                    write!(f, "$")?;
                }
                write!(f, "{}{}", col_to_letters(*col), "")?;
                if *abs_row {
                    write!(f, "$")?;
                }
                write!(f, "{}", row + 1)
            }
            Token::Plus => write!(f, "+"),
            Token::Dash => write!(f, "-"),
            Token::Star => write!(f, "*"),
            Token::Slash => write!(f, "/"),
            Token::Dot => write!(f, "."),
            Token::At => write!(f, "@"),
            Token::Colon => write!(f, ":"),
            Token::Comma => write!(f, ","),
            Token::LParen => write!(f, "("),
            Token::RParen => write!(f, ")"),
            Token::Eq => write!(f, "="),
            Token::Gt => write!(f, ">"),
            Token::Lt => write!(f, "<"),
            Token::Backslash => write!(f, "\\"),
            Token::Arrow => write!(f, "->"),
            Token::Text(s) => write!(f, "{s}"),
        }
    }
}

type Spanned<T> = chumsky::span::Spanned<T, SimpleSpan>;

// todo: refactor?
const BUILTINS: &[&str] = &["sum", "avg", "min", "max", "count", "if"];

// --- Error utilities ---

fn build_report(title: &str, src: &str, span: std::ops::Range<usize>, message: String) -> String {
    use ariadne::{Config, IndexType, Label, Report, ReportKind, Source};

    let src_id = "";
    let mut buf = Vec::new();
    Report::build(ReportKind::Error, (src_id, span.clone()))
        .with_config(
            Config::new()
                .with_color(false)
                .with_index_type(IndexType::Byte)
                .with_compact(true),
        )
        .with_label(Label::new((src_id, span)).with_message(message))
        .finish()
        .write((src_id, Source::from(format!("={src}"))), &mut buf)
        .unwrap();
    let output = String::from_utf8(buf).unwrap();
    let body: String = output
        .lines()
        .filter(|line| !line.starts_with("Error") && !line.contains("─["))
        .collect::<Vec<_>>()
        .join("\n");
    format!("{title}\n{body}")
}

fn format_parsing_error(src: &str, span: std::ops::Range<usize>, msg: &str) -> String {
    build_report("Parsing error:", src, span, msg.to_string())
}

fn uppercase_builtin_message(src: &str, paren: usize) -> Option<(std::ops::Range<usize>, String)> {
    if src.as_bytes().get(paren) != Some(&b'(') {
        return None;
    }

    let mut start = paren;
    while start > 0 && src.as_bytes()[start - 1].is_ascii_alphabetic() {
        start -= 1;
    }

    let name = &src[start..paren];
    let lowercase_name = name.to_ascii_lowercase();
    if lowercase_name == name || !BUILTINS.contains(&lowercase_name.as_str()) {
        return None;
    }

    Some((
        start + 1..paren + 1,
        format!("Unknown function, did you mean '{lowercase_name}'? (All functions in Tonic are lowercase)"),
    ))
}

pub fn format_lex_error(src: &str, err: &Rich<'_, char>) -> String {
    let s = err.span().into_range();
    format_parsing_error(src, s.start + 1..s.end + 1, &err.to_string())
}

pub fn format_parse_error<'src>(src: &str, err: &Rich<'_, Token<'src>>) -> String {
    let s = err.span().into_range();
    if let Some((span, message)) = uppercase_builtin_message(src, s.start) {
        return format_parsing_error(src, span, &message);
    }
    format_parsing_error(src, s.start + 1..s.end + 1, &err.to_string())
}

pub fn format_eval_error(formula_string: &str, message: &str, span: Option<(u32, u32)>) -> String {
    let src = if formula_string.starts_with('=') {
        &formula_string[1..]
    } else {
        formula_string
    };
    // Spans are relative to formula_string[1..] (without '=').
    // build_report writes "={src}", so add 1 to align with the '=' prefix.
    let ariadne_span = match span {
        Some((start, end)) => (start + 1) as usize..(end + 1) as usize,
        None => 1..formula_string.len(),
    };
    build_report("Type error:", src, ariadne_span, message.to_string())
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
        chumsky::regex::regex("\\$?[a-zA-Z]{1,2}\\$?[0-9]+").map(lex_cell_ref),
        text::ident().map(|s| match s {
            "true" => Token::True,
            "false" => Token::False,
            s => Token::Name(s),
        }),
        just('+').to(Token::Plus),
        just('=').to(Token::Eq),
        just('>').to(Token::Gt),
        just('<').to(Token::Lt),
        just('\\').to(Token::Backslash),
        just("->").to(Token::Arrow),
        just('-').to(Token::Dash),
        just('*').to(Token::Star),
        just('/').to(Token::Slash),
        just('.').to(Token::Dot),
        just('@').to(Token::At),
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
            .try_map(|s: &str, span| {
                s.parse::<Decimal>()
                    .map(Token::Number)
                    .map_err(|_| Rich::custom(span, "number is too large"))
            }),
    ))
    .spanned()
    .padded()
    .repeated()
    .collect()
}

fn lex_cell_ref(s: &str) -> Token<'_> {
    let bytes = s.as_bytes();
    let mut i = 0;
    let abs_col = if bytes[i] == b'$' {
        i += 1;
        true
    } else {
        false
    };

    let mut col = 0u32;
    while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
        col = col * 26 + (bytes[i].to_ascii_uppercase() - b'A') as u32 + 1;
        i += 1;
    }
    col -= 1;

    let abs_row = if i < bytes.len() && bytes[i] == b'$' {
        i += 1;
        true
    } else {
        false
    };

    let mut row = 0u32;
    while i < bytes.len() {
        row = row * 10 + (bytes[i] - b'0') as u32;
        i += 1;
    }

    Token::Cell {
        col,
        row: row - 1,
        abs_col,
        abs_row,
    }
}

fn make_coordinate(value: u32, absolute: bool, base: u32) -> Coordinate {
    if absolute {
        Coordinate::Absolute(value)
    } else {
        Coordinate::Relative(value as i32 - base as i32)
    }
}

fn format_cell_ref(id: &AbsoluteCellId, abs_col: bool, abs_row: bool) -> String {
    let mut result = String::new();
    if abs_col {
        result.push('$');
    }
    result.push_str(&col_to_letters(id.col));
    if abs_row {
        result.push('$');
    }
    result.push_str(&(id.row + 1).to_string());
    result
}

fn col_to_letters(col: u32) -> String {
    let mut result = String::new();
    let mut c = col;
    loop {
        result.insert(0, (b'A' + (c % 26) as u8) as char);
        if c < 26 {
            break;
        }
        c = c / 26 - 1;
    }
    result
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
    pub current_table_id: Option<TableId>,
    pub expr_arena: Vec<Expr>,
    pub span_arena: Vec<(u32, u32)>,
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

fn push_expr(state: &mut FormulaState, expr: Expr, span: SimpleSpan) -> ExprId {
    let id = state.expr_arena.len() as ExprId;
    state.expr_arena.push(expr);
    let r = span.into_range();
    state.span_arena.push((r.start as u32, r.end as u32));
    id
}

/// Parser for formula language. Resolves all references (named cells, functions, sheets)
fn create_formula_praser<'tokens, 'src: 'tokens>(
) -> impl Parser<'tokens, FormulaInput<'tokens, 'src>, FormulaOutput, FormulaExtra<'tokens, 'src>>
       + 'tokens {
    // define recursive parser (use 'expr' to reference to itself)
    recursive(|expr| {
        let ident = select_ref! { Token::Name(s) => *s };

        let cell_ref = select_ref! {
            Token::Cell { col, row, abs_col, abs_row } => (*col, *row, *abs_col, *abs_row)
        };

        // call_args: (expr, expr, ...)
        let call_args = expr
            .clone()
            .separated_by(just(Token::Comma))
            .collect::<Vec<ExprId>>()
            .delimited_by(just(Token::LParen), just(Token::RParen));

        // Table.Column == table column body range
        let table_column_ref = ident
            .clone()
            .then_ignore(just(Token::Dot))
            .then(ident.clone())
            .try_map_with(|(table_name, column_name), extra| {
                let span = extra.span();
                let st: &mut FormulaState = extra.state();
                let table_id = st.names.table_names.get(table_name).ok_or_else(|| {
                    Rich::custom(
                        span,
                        format!("Unresolved name '{table_name}.{column_name}'"),
                    )
                })?;
                let reference = st
                    .names
                    .table_columns
                    .get(&(*table_id, column_name.to_string()))
                    .ok_or_else(|| {
                        Rich::custom(
                            span,
                            format!("Unresolved name '{table_name}.{column_name}'"),
                        )
                    })?
                    .clone();
                Ok(push_expr(
                    extra.state(),
                    Expr::Atom(ExprAtom::Reference(reference)),
                    span,
                ))
            });

        // @Column == current row in table column
        let current_row_column_ref =
            just(Token::At)
                .ignore_then(ident.clone())
                .try_map_with(|column_name, extra| {
                    let span = extra.span();
                    let st: &mut FormulaState = extra.state();
                    // @Column only makes sense from inside the table
                    let Some(table_id) = st.current_table_id else {
                        return Err(Rich::custom(
                            span,
                            format!("Column reference '@{column_name}' is invalid outside tables"),
                        ));
                    };
                    let reference = st
                        .names
                        .table_columns
                        .get(&(table_id, column_name.to_string()))
                        .ok_or_else(|| {
                            Rich::custom(span, format!("Unresolved name '@{column_name}'"))
                        })?;
                    let Reference::Range {
                        sheet_id,
                        start_col,
                        ..
                    } = reference
                    else {
                        return Err(Rich::custom(
                            span,
                            format!("Unresolved name '@{column_name}'"),
                        ));
                    };
                    let sheet_id = *sheet_id;
                    let col = Coordinate::Absolute(start_col.to_index(st.cell_id.col));
                    // row stays relative so shared formulas keep current-row behavior
                    Ok(push_expr(
                        extra.state(),
                        Expr::Atom(ExprAtom::Reference(Reference::Single {
                            sheet_id,
                            row: Coordinate::Relative(0),
                            col,
                        })),
                        span,
                    ))
                });

        // cell_name_or_function_call == name followed by optional (...) for function call, otherwise named reference
        let cell_name_or_function_call =
            ident
                .clone()
                .then(call_args.or_not())
                .try_map_with(|(name, args), extra| {
                    let span = extra.span();
                    let st: &mut FormulaState = extra.state();
                    let expr = if let Some(args) = args {
                        // function call: name(...)
                        // todo: abstract the function defenition
                        match name {
                            "sum" => {
                                if args.len() != 1 {
                                    return Err(Rich::custom(
                                        span,
                                        "sum expects exactly 1 argument",
                                    ));
                                }
                                Expr::Sum(args[0])
                            }
                            "avg" => {
                                if args.len() != 1 {
                                    return Err(Rich::custom(
                                        span,
                                        "avg expects exactly 1 argument",
                                    ));
                                }
                                Expr::Avg(args[0])
                            }
                            "min" => {
                                if args.len() != 1 {
                                    return Err(Rich::custom(
                                        span,
                                        "min expects exactly 1 argument",
                                    ));
                                }
                                Expr::Min(args[0])
                            }
                            "max" => {
                                if args.len() != 1 {
                                    return Err(Rich::custom(
                                        span,
                                        "max expects exactly 1 argument",
                                    ));
                                }
                                Expr::Max(args[0])
                            }
                            "count" => {
                                if args.len() != 2 {
                                    return Err(Rich::custom(
                                        span,
                                        "count expects exactly 2 arguments",
                                    ));
                                }
                                Expr::Count(args[0], args[1])
                            }
                            "if" => {
                                if args.len() != 3 {
                                    return Err(Rich::custom(
                                        span,
                                        "if expects exactly 3 arguments",
                                    ));
                                }
                                Expr::If(args[0], args[1], args[2])
                            }
                            _ => {
                                let lowercase_name = name.to_ascii_lowercase();
                                // help users who typed uppercase functions instead of lowercase (SUM, AVG, etc).
                                if lowercase_name != name && BUILTINS.contains(&lowercase_name.as_str()) {
                                    return Err(Rich::custom(
                                        span,
                                        format!("Unknown function, did you mean '{lowercase_name}'? (All functions in Tonic are lowercase)"),
                                    ));
                                }
                                // look up user-registered JS function
                                let func_id =
                                    st.names.user_function_names.get(name).ok_or_else(|| {
                                        Rich::custom(span, format!("unresolved function '{name}'"))
                                    })?;
                                Expr::ExternalFunctionCall {
                                    func_id: *func_id,
                                    args,
                                }
                            }
                        }
                    } else {
                        // todo: remove hardcode fix below.
                        // bare name: check if it's a known function missing parens
                        if BUILTINS.contains(&name)
                            || st.names.user_function_names.contains_key(name)
                        {
                            return Err(Rich::custom(
                                span,
                                format!("'{name}' is a function, expected function arguments"),
                            ));
                        }
                        if let Some(table_id) = st.current_table_id {
                            if let Some(reference) =
                                st.names.table_columns.get(&(table_id, name.to_string()))
                            {
                                let reference = reference.clone();
                                return Ok(push_expr(
                                    extra.state(),
                                    Expr::Atom(ExprAtom::Reference(reference)),
                                    span,
                                ));
                            }
                        }
                        let named_cell = st.names.cell_names.get(name).ok_or_else(|| {
                            Rich::custom(span, format!("Unresolved name '{name}'"))
                        })?;
                        Expr::Atom(ExprAtom::Reference(Reference::Single {
                            sheet_id: named_cell.sheet_id,
                            row: Coordinate::Absolute(named_cell.row),
                            col: Coordinate::Absolute(named_cell.col),
                        }))
                    };
                    let span = extra.span();
                    Ok(push_expr(extra.state(), expr, span))
                });

        // A1 or A1:B2 == produces Reference with offsets
        let cell_or_range = cell_ref
            .clone()
            .then(just(Token::Colon).ignore_then(cell_ref.clone()).or_not())
            .map_with(|(start, end), extra| {
                let st: &mut FormulaState = extra.state();
                let start_row = make_coordinate(start.1, start.3, st.cell_id.row);
                let start_col = make_coordinate(start.0, start.2, st.cell_id.col);
                match end {
                    Some(end) => {
                        let end_row = make_coordinate(end.1, end.3, st.cell_id.row);
                        let end_col = make_coordinate(end.0, end.2, st.cell_id.col);
                        ExprAtom::Reference(Reference::Range {
                            sheet_id: 0,
                            start_row,
                            start_col,
                            end_row,
                            end_col,
                        })
                    }
                    None => ExprAtom::Reference(Reference::Single {
                        sheet_id: 0,
                        row: start_row,
                        col: start_col,
                    }),
                }
            });

        // atom_value == self-contained value (3, false, A1, A1:A5, 3.14, "text", etc)
        let atom_value = choice((
            select_ref! { Token::True => ExprAtom::Bool(true) },
            select_ref! { Token::False => ExprAtom::Bool(false) },
            select_ref! { Token::Number(x) => ExprAtom::Number(*x) },
            select_ref! { Token::Text(s) => ExprAtom::Text(s.to_string()) },
            cell_or_range,
        ))
        .map_with(|val, extra| {
            let span = extra.span();
            push_expr(extra.state(), Expr::Atom(val), span)
        });

        // parenthesized expression: ( expr )
        let paren_expr = expr
            .clone()
            .delimited_by(just(Token::LParen), just(Token::RParen));

        let atom = choice((
            table_column_ref,
            current_row_column_ref,
            cell_name_or_function_call,
            paren_expr,
            atom_value,
        ));

        atom.pratt((
            prefix(3, just(Token::Dash), |_, r, e| {
                let span = e.span();
                push_expr(e.state(), Expr::Negate(r), span)
            }),
            infix(left(2), just(Token::Star), |l, _, r, e| {
                let span = e.span();
                push_expr(e.state(), Expr::Multiply(l, r), span)
            }),
            infix(left(2), just(Token::Slash), |l, _, r, e| {
                let span = e.span();
                push_expr(e.state(), Expr::Divide(l, r), span)
            }),
            infix(left(1), just(Token::Plus), |l, _, r, e| {
                let span = e.span();
                push_expr(e.state(), Expr::Add(l, r), span)
            }),
            infix(left(1), just(Token::Dash), |l, _, r, e| {
                let span = e.span();
                push_expr(e.state(), Expr::Subtract(l, r), span)
            }),
            infix(left(0), just(Token::Eq), |l, _, r, e| {
                let span = e.span();
                push_expr(e.state(), Expr::Equal(l, r), span)
            }),
            infix(left(0), just(Token::Gt), |l, _, r, e| {
                let span = e.span();
                push_expr(e.state(), Expr::GreaterThan(l, r), span)
            }),
            infix(left(0), just(Token::Lt), |l, _, r, e| {
                let span = e.span();
                push_expr(e.state(), Expr::LessThan(l, r), span)
            }),
        ))
    })
}

/// Parse a formula (already lexed) into a flat Vec<Expr> arena.
/// Returns the arena, span table, and the root ExprId, or errors.
pub fn parse_formula<'tokens, 'src: 'tokens>(
    tokens: &'tokens [Spanned<Token<'src>>],
    src_len: usize,
    state: &mut FormulaState<'tokens>,
) -> (
    Option<(Vec<Expr>, Vec<(u32, u32)>, ExprId)>,
    Vec<Rich<'tokens, Token<'src>>>,
) {
    let eoi = SimpleSpan::new((), src_len..src_len);
    let result = create_formula_praser()
        .boxed()
        .parse_with_state(tokens.split_spanned(eoi), state);
    let errs: Vec<_> = result.errors().cloned().collect();
    let output = result.into_output().map(|root| {
        (
            std::mem::take(&mut state.expr_arena),
            std::mem::take(&mut state.span_arena),
            root,
        )
    });
    (output, errs)
}

/// Build formula template slots from parser spans.
pub fn create_formula_template_refs(
    ast: &[Expr],
    spans: &[(u32, u32)],
    span_offset: u32,
) -> Vec<FormulaTemplateRef> {
    let mut refs = Vec::new();
    for (expr_id, expr) in ast.iter().enumerate() {
        // only reference text is dynamic in the displayed formula
        let Expr::Atom(ExprAtom::Reference(_) | ExprAtom::InvalidReferenceError(_)) = expr else {
            continue;
        };
        let Some((start, end)) = spans.get(expr_id).copied() else {
            continue;
        };
        // spans come from formula text without '='
        refs.push(FormulaTemplateRef {
            expr_id: expr_id as ExprId,
            start: start + span_offset,
            end: end + span_offset,
        });
    }
    refs.sort_unstable_by_key(|r| r.start);
    refs
}

/// Create editor text from formula template.
///
/// Constant spans stay as typed. Reference spans are created from AST.
pub fn create_formula_string(
    formula: &Formula,
    cell_id: &GridCellId,
    names: &SpreadsheetNames,
) -> String {
    let source_cell = AbsoluteCellId {
        sheet_id: 0,
        row: cell_id.row,
        col: cell_id.col,
    };
    let source_range = CellRange::single(source_cell);
    let source_table = names
        .table_columns_lookup
        .iter()
        .find_map(|(range, (table_id, _))| {
            range.contains(&source_range).then_some((*table_id, *range))
        });
    let template = &formula.formula_string_template;
    if formula.template_refs.is_empty() {
        return template.clone();
    }

    let create_reference_string = |reference: &Reference| -> String {
        let range = reference.to_cell_range(&source_cell);

        // same-row table references are displayed as @Column
        if range.is_single() && range.start_row == source_cell.row {
            if let Some((table_id, table_range)) = source_table {
                let column_range = CellRange::new(
                    table_range.sheet_id,
                    table_range.start_row,
                    range.start_col,
                    table_range.end_row,
                    range.start_col,
                );
                if let Some((column_table_id, name)) = names.table_columns_lookup.get(&column_range)
                {
                    if *column_table_id == table_id {
                        return format!("@{name}");
                    }
                }
            }
        }

        // full table-column ranges are displayed with column names
        if let Some((table_id, column_name)) = names.table_columns_lookup.get(&range) {
            if source_table.is_some_and(|(source_table_id, _)| source_table_id == *table_id) {
                return column_name.clone();
            }
            if let Some(table_name) = names.table_names_lookup.get(table_id) {
                return format!("{table_name}.{column_name}");
            }
        }

        // otherwise, create regular A1 references from the resolved coordinates
        match reference {
            Reference::Single { row, col, .. } => format_cell_ref(
                &range.head_cell(),
                matches!(col, Coordinate::Absolute(_)),
                matches!(row, Coordinate::Absolute(_)),
            ),
            Reference::Range {
                sheet_id,
                start_row,
                start_col,
                end_row,
                end_col,
            } => {
                let start_id = AbsoluteCellId {
                    sheet_id: *sheet_id,
                    row: start_row.to_index(source_cell.row),
                    col: start_col.to_index(source_cell.col),
                };
                let end_id = AbsoluteCellId {
                    sheet_id: *sheet_id,
                    row: end_row.to_index(source_cell.row),
                    col: end_col.to_index(source_cell.col),
                };
                let mut text = format_cell_ref(
                    &start_id,
                    matches!(start_col, Coordinate::Absolute(_)),
                    matches!(start_row, Coordinate::Absolute(_)),
                );
                text.push(':');
                text.push_str(&format_cell_ref(
                    &end_id,
                    matches!(end_col, Coordinate::Absolute(_)),
                    matches!(end_row, Coordinate::Absolute(_)),
                ));
                text
            }
        }
    };

    let mut result = String::with_capacity(template.len());
    let mut pos = 0usize;
    for template_ref in &formula.template_refs {
        let start = template_ref.start as usize;
        let end = template_ref.end as usize;
        if start < pos || end > template.len() {
            continue;
        }

        // copy constant text, then create the reference slot from AST
        result.push_str(&template[pos..start]);
        match formula.ast.get(template_ref.expr_id as usize) {
            Some(Expr::Atom(ExprAtom::Reference(reference))) => {
                result.push_str(&create_reference_string(reference));
            }
            Some(Expr::Atom(ExprAtom::InvalidReferenceError(msg))) => {
                result.push_str(msg);
            }
            _ => result.push_str(&template[start..end]),
        }
        pos = end;
    }
    result.push_str(&template[pos..]);
    result
}

/// Returns true if the string matches a regular cell name like "A1", "Z10", "AA5", "ZZ100".
pub fn string_is_regular_cell_name(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    let mut i = 0;
    if bytes[i] == b'$' {
        i += 1;
    }
    let col_start = i;
    while i < bytes.len() && bytes[i].is_ascii_alphabetic() && i - col_start < 2 {
        i += 1;
    }
    if i == col_start || i == bytes.len() || (i < bytes.len() && bytes[i].is_ascii_alphabetic()) {
        return false;
    }
    if i < bytes.len() && bytes[i] == b'$' {
        i += 1;
    }
    if i == bytes.len() {
        return false;
    }
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            return false;
        }
        i += 1;
    }
    true
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

    const TABLE_ID: TableId = 1;
    const TABLE_NAME: &str = "Table1";

    /// Lex + parse a formula string, return (arena, root_id) or panic with errors.
    fn parse(src: &str, names: &mut SpreadsheetNames) -> (Vec<Expr>, ExprId) {
        let tokens = lex_formula(src).into_output().expect("lexer failed");
        let current_table_id = names.table_names.get(TABLE_NAME).copied();
        let mut state = FormulaState {
            names,
            cell_id: GridCellId { col: 0, row: 0 },
            current_table_id,
            expr_arena: Vec::new(),
            span_arena: Vec::new(),
        };
        let (parsed, errs) = parse_formula(&tokens, src.len(), &mut state);
        assert!(
            errs.is_empty(),
            "parse errors: {}",
            errs.iter()
                .map(|e| format_parse_error(src, e))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let (arena, _spans, root) = parsed.expect("no output");
        (arena, root)
    }

    fn assert_parses(cases: &[(&str, Vec<Expr>)]) {
        let mut names = SpreadsheetNames::new();
        for (src, expected_arena) in cases {
            let (arena, _root) = parse(src, &mut names);
            assert_eq!(arena, *expected_arena, "failed for input: {src:?}");
        }
    }

    fn parse_error(src: &str) -> String {
        let tokens = lex_formula(src).into_output().expect("lexer failed");
        let mut names = SpreadsheetNames::new();
        let mut state = FormulaState {
            names: &mut names,
            cell_id: GridCellId { col: 0, row: 0 },
            current_table_id: None,
            expr_arena: Vec::new(),
            span_arena: Vec::new(),
        };
        let (_, errs) = parse_formula(&tokens, src.len(), &mut state);
        assert!(!errs.is_empty(), "expected parse error");
        format_parse_error(src, &errs[0])
    }

    fn r(col: i32, row: i32) -> Reference {
        Reference::Single {
            sheet_id: 0,
            row: Coordinate::Relative(row),
            col: Coordinate::Relative(col),
        }
    }

    fn a(col: u32, row: u32) -> Reference {
        Reference::Single {
            sheet_id: 0,
            row: Coordinate::Absolute(row),
            col: Coordinate::Absolute(col),
        }
    }

    fn table_column() -> Reference {
        Reference::Range {
            sheet_id: 0,
            start_row: Coordinate::Absolute(1),
            start_col: Coordinate::Absolute(2),
            end_row: Coordinate::Absolute(3),
            end_col: Coordinate::Absolute(2),
        }
    }

    #[test]
    fn test_literals() {
        assert_parses(&[
            ("42", vec![Expr::Atom(ExprAtom::Number(dec!(42)))]),
            ("3.14", vec![Expr::Atom(ExprAtom::Number(dec!(3.14)))]),
            ("true", vec![Expr::Atom(ExprAtom::Bool(true))]),
            ("false", vec![Expr::Atom(ExprAtom::Bool(false))]),
            (
                r#""hello""#,
                vec![Expr::Atom(ExprAtom::Text("hello".into()))],
            ),
        ]);
    }

    #[test]
    fn test_cell_ref() {
        assert_parses(&[
            ("A1", vec![Expr::Atom(ExprAtom::Reference(r(0, 0)))]),
            ("B3", vec![Expr::Atom(ExprAtom::Reference(r(1, 2)))]),
            ("$A$1", vec![Expr::Atom(ExprAtom::Reference(a(0, 0)))]),
            (
                "A$2",
                vec![Expr::Atom(ExprAtom::Reference(Reference::Single {
                    sheet_id: 0,
                    row: Coordinate::Absolute(1),
                    col: Coordinate::Relative(0),
                }))],
            ),
        ]);
    }

    #[test]
    fn test_cell_range() {
        assert_parses(&[(
            "A1:B2",
            vec![Expr::Atom(ExprAtom::Reference(Reference::Range {
                sheet_id: 0,
                start_row: Coordinate::Relative(0),
                start_col: Coordinate::Relative(0),
                end_row: Coordinate::Relative(1),
                end_col: Coordinate::Relative(1),
            }))],
        )]);
        assert_parses(&[(
            "$A1:B$2",
            vec![Expr::Atom(ExprAtom::Reference(Reference::Range {
                sheet_id: 0,
                start_row: Coordinate::Relative(0),
                start_col: Coordinate::Absolute(0),
                end_row: Coordinate::Absolute(1),
                end_col: Coordinate::Relative(1),
            }))],
        )]);
    }

    #[test]
    fn test_arithmetic() {
        assert_parses(&[
            (
                "1 + 2",
                vec![
                    Expr::Atom(ExprAtom::Number(dec!(1))),
                    Expr::Atom(ExprAtom::Number(dec!(2))),
                    Expr::Add(0, 1),
                ],
            ),
            (
                "5 - 3",
                vec![
                    Expr::Atom(ExprAtom::Number(dec!(5))),
                    Expr::Atom(ExprAtom::Number(dec!(3))),
                    Expr::Subtract(0, 1),
                ],
            ),
            (
                "2 * 3",
                vec![
                    Expr::Atom(ExprAtom::Number(dec!(2))),
                    Expr::Atom(ExprAtom::Number(dec!(3))),
                    Expr::Multiply(0, 1),
                ],
            ),
            (
                "-1",
                vec![Expr::Atom(ExprAtom::Number(dec!(1))), Expr::Negate(0)],
            ),
        ]);
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
    fn test_builtin_functions() {
        let range = Expr::Atom(ExprAtom::Reference(Reference::Range {
            sheet_id: 0,
            start_row: Coordinate::Relative(0),
            start_col: Coordinate::Relative(0),
            end_row: Coordinate::Relative(1),
            end_col: Coordinate::Relative(1),
        }));
        assert_parses(&[
            ("sum(A1:B2)", vec![range.clone(), Expr::Sum(0)]),
            ("avg(A1:B2)", vec![range, Expr::Avg(0)]),
        ]);
    }

    #[test]
    fn uppercase_builtin_function_suggests_lowercase() {
        let error = parse_error("SUM(A1:B2)");
        assert!(
            error.contains(
                "Unknown function, did you mean 'sum'? (All functions in Tonic are lowercase)"
            ),
            "{error}"
        );
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
                row: Coordinate::Absolute(10),
                col: Coordinate::Absolute(5),
            }))]
        );
    }

    #[test]
    fn test_table_column_refs() {
        let mut names = SpreadsheetNames::new();
        let reference = table_column();
        names.table_names.insert(TABLE_NAME.into(), TABLE_ID);
        names
            .table_columns
            .insert((TABLE_ID, "Column_Name".into()), reference.clone());

        let (arena, _) = parse("Table1.Column_Name", &mut names);
        assert_eq!(
            arena,
            vec![Expr::Atom(ExprAtom::Reference(reference.clone()))]
        );

        let (arena, _) = parse("Column_Name", &mut names);
        assert_eq!(arena, vec![Expr::Atom(ExprAtom::Reference(reference))]);
    }

    #[test]
    fn test_current_row_table_column_ref() {
        let mut names = SpreadsheetNames::new();
        names.table_names.insert(TABLE_NAME.into(), TABLE_ID);
        names
            .table_columns
            .insert((TABLE_ID, "Column_Name".into()), table_column());

        let (arena, _) = parse("@Column_Name", &mut names);
        assert_eq!(
            arena,
            vec![Expr::Atom(ExprAtom::Reference(Reference::Single {
                sheet_id: 0,
                row: Coordinate::Relative(0),
                col: Coordinate::Absolute(2),
            }))]
        );
    }

    #[test]
    fn create_formula_string_replaces_invalid_reference_with_error_marker() {
        let names = SpreadsheetNames::new();
        let ast = vec![
            Expr::Atom(ExprAtom::InvalidReferenceError("#REF!".into())),
            Expr::Atom(ExprAtom::Number(dec!(5))),
            Expr::Add(0, 1),
        ];
        let spans = vec![(0, 2), (3, 4), (0, 4)];
        let template_refs = create_formula_template_refs(&ast, &spans, 1);
        let formula = Formula {
            ast,
            formula_string_template: "=A1+5".into(),
            template_refs,
            spans,
        };

        let text = create_formula_string(&formula, &GridCellId { row: 0, col: 0 }, &names);
        assert_eq!(text, "=#REF!+5");
    }
}
