//! A small, safe expression language for rule checks.
//!
//! Grammar (lowest to highest precedence):
//! `||`, `&&`, comparisons (`== != < <= > >=`), `+ -`, `* /`, unary `! -`,
//! then literals (numbers, `"strings"`, `true`/`false`), dotted names
//! (`width`, `section.b`, `param.minClear`), calls (`min`, `max`, `abs`)
//! and parentheses. Expressions are parsed once when a rule pack loads.

use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    Num(f64),
    Bool(bool),
    Str(String),
}

impl fmt::Display for Val {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Val::Num(n) => write!(f, "{n}"),
            Val::Bool(b) => write!(f, "{b}"),
            Val::Str(s) => write!(f, "\"{s}\""),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Lit(Val),
    Name(String),
    Unary(char, Box<Expr>),
    Binary(String, Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ExprError {
    #[error("syntax error at {pos}: {msg}")]
    Syntax { pos: usize, msg: String },
    #[error("missing value '{0}'")]
    Missing(String),
    #[error("type error: {0}")]
    Type(String),
}

/// Supplies values for names during evaluation.
pub trait Scope {
    fn lookup(&self, name: &str) -> Option<Val>;
}

// ---------- lexer ----------

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Str(String),
    Ident(String),
    Op(String),
    LParen,
    RParen,
    Comma,
}

fn lex(src: &str) -> Result<Vec<(usize, Tok)>, ExprError> {
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    let err = |pos, msg: &str| ExprError::Syntax { pos, msg: msg.into() };
    while i < chars.len() {
        let c = chars[i];
        let start = i;
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_digit() || (c == '.' && chars.get(i + 1).is_some_and(|d| d.is_ascii_digit())) {
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            let s: String = chars[start..i].iter().collect();
            out.push((start, Tok::Num(s.parse().map_err(|_| err(start, "bad number"))?)));
        } else if c.is_alphabetic() || c == '_' {
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '.') {
                i += 1;
            }
            out.push((start, Tok::Ident(chars[start..i].iter().collect())));
        } else if c == '"' {
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                i += 1;
            }
            if i >= chars.len() {
                return Err(err(start, "unterminated string"));
            }
            out.push((start, Tok::Str(chars[start + 1..i].iter().collect())));
            i += 1;
        } else {
            let two: String = chars[i..(i + 2).min(chars.len())].iter().collect();
            let tok = match two.as_str() {
                "&&" | "||" | "==" | "!=" | "<=" | ">=" => {
                    i += 2;
                    Tok::Op(two)
                }
                _ => {
                    i += 1;
                    match c {
                        '(' => Tok::LParen,
                        ')' => Tok::RParen,
                        ',' => Tok::Comma,
                        '+' | '-' | '*' | '/' | '<' | '>' | '!' => Tok::Op(c.to_string()),
                        _ => return Err(err(start, &format!("unexpected '{c}'"))),
                    }
                }
            };
            out.push((start, tok));
        }
    }
    Ok(out)
}

// ---------- parser ----------

/// Limits so a user-supplied rule pack cannot overflow the stack (recursion) or exhaust memory.
const MAX_DEPTH: usize = 256;
const MAX_TOKENS: usize = 2_000;

struct Parser {
    depth: usize,
    toks: Vec<(usize, Tok)>,
    i: usize,
    len: usize,
}

const LEVELS: &[&[&str]] = &[&["||"], &["&&"], &["==", "!=", "<", "<=", ">", ">="], &["+", "-"], &["*", "/"]];

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.i).map(|t| &t.1)
    }
    fn pos(&self) -> usize {
        self.toks.get(self.i).map_or(self.len, |t| t.0)
    }
    fn fail<T>(&self, msg: &str) -> Result<T, ExprError> {
        Err(ExprError::Syntax { pos: self.pos(), msg: msg.into() })
    }

    fn binary(&mut self, level: usize) -> Result<Expr, ExprError> {
        if level == LEVELS.len() {
            return self.unary();
        }
        let mut lhs = self.binary(level + 1)?;
        while let Some(Tok::Op(op)) = self.peek() {
            if !LEVELS[level].contains(&op.as_str()) {
                break;
            }
            let op = op.clone();
            self.i += 1;
            let rhs = self.binary(level + 1)?;
            lhs = Expr::Binary(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn enter(&mut self) -> Result<(), ExprError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return self.fail("expression is nested too deeply");
        }
        Ok(())
    }

    fn unary(&mut self) -> Result<Expr, ExprError> {
        self.enter()?;
        let r = self.unary_inner();
        self.depth -= 1;
        r
    }

    fn primary(&mut self) -> Result<Expr, ExprError> {
        self.enter()?;
        let r = self.primary_inner();
        self.depth -= 1;
        r
    }

    fn unary_inner(&mut self) -> Result<Expr, ExprError> {
        if let Some(Tok::Op(op)) = self.peek()
            && (op == "!" || op == "-") {
                let c = op.chars().next().unwrap();
                self.i += 1;
                return Ok(Expr::Unary(c, Box::new(self.unary()?)));
            }
        self.primary()
    }

    fn primary_inner(&mut self) -> Result<Expr, ExprError> {
        let Some(tok) = self.peek().cloned() else { return self.fail("unexpected end") };
        self.i += 1;
        match tok {
            Tok::Num(n) => Ok(Expr::Lit(Val::Num(n))),
            Tok::Str(s) => Ok(Expr::Lit(Val::Str(s))),
            Tok::Ident(id) if id == "true" => Ok(Expr::Lit(Val::Bool(true))),
            Tok::Ident(id) if id == "false" => Ok(Expr::Lit(Val::Bool(false))),
            Tok::Ident(id) => {
                if self.peek() != Some(&Tok::LParen) {
                    return Ok(Expr::Name(id));
                }
                if !matches!(id.as_str(), "min" | "max" | "abs") {
                    self.i -= 1;
                    return self.fail(&format!("unknown function '{id}'"));
                }
                self.i += 1;
                let mut args = Vec::new();
                if self.peek() != Some(&Tok::RParen) {
                    loop {
                        args.push(self.binary(0)?);
                        if self.peek() == Some(&Tok::Comma) {
                            self.i += 1;
                        } else {
                            break;
                        }
                    }
                }
                if self.peek() != Some(&Tok::RParen) {
                    return self.fail("expected ')'");
                }
                self.i += 1;
                Ok(Expr::Call(id, args))
            }
            Tok::LParen => {
                let e = self.binary(0)?;
                if self.peek() != Some(&Tok::RParen) {
                    return self.fail("expected ')'");
                }
                self.i += 1;
                Ok(e)
            }
            _ => {
                self.i -= 1;
                self.fail("unexpected token")
            }
        }
    }
}

pub fn parse(src: &str) -> Result<Expr, ExprError> {
    let toks = lex(src)?;
    if toks.len() > MAX_TOKENS {
        return Err(ExprError::Syntax { pos: 0, msg: "expression is too long".into() });
    }
    let mut p = Parser { depth: 0, toks, i: 0, len: src.chars().count() };
    let e = p.binary(0)?;
    if p.i != p.toks.len() {
        return p.fail("unexpected trailing input");
    }
    Ok(e)
}

// ---------- evaluator ----------

fn num(v: Val) -> Result<f64, ExprError> {
    match v {
        Val::Num(n) => Ok(n),
        o => Err(ExprError::Type(format!("expected number, got {o}"))),
    }
}

fn boolean(v: Val) -> Result<bool, ExprError> {
    match v {
        Val::Bool(b) => Ok(b),
        o => Err(ExprError::Type(format!("expected boolean, got {o}"))),
    }
}

impl Expr {
    pub fn eval(&self, scope: &dyn Scope) -> Result<Val, ExprError> {
        Ok(match self {
            Expr::Lit(v) => v.clone(),
            Expr::Name(n) => scope.lookup(n).ok_or_else(|| ExprError::Missing(n.clone()))?,
            Expr::Unary('!', e) => Val::Bool(!boolean(e.eval(scope)?)?),
            Expr::Unary(_, e) => Val::Num(-num(e.eval(scope)?)?),
            Expr::Binary(op, a, b) => match op.as_str() {
                // Short-circuit so `a && b` skips b when a is false.
                "&&" => Val::Bool(boolean(a.eval(scope)?)? && boolean(b.eval(scope)?)?),
                "||" => Val::Bool(boolean(a.eval(scope)?)? || boolean(b.eval(scope)?)?),
                "==" => Val::Bool(a.eval(scope)? == b.eval(scope)?),
                "!=" => Val::Bool(a.eval(scope)? != b.eval(scope)?),
                _ => {
                    let (x, y) = (num(a.eval(scope)?)?, num(b.eval(scope)?)?);
                    match op.as_str() {
                        "+" => Val::Num(x + y),
                        "-" => Val::Num(x - y),
                        "*" => Val::Num(x * y),
                        "/" if y == 0.0 => return Err(ExprError::Type("division by zero".into())),
                        "/" => Val::Num(x / y),
                        // Tolerance absorbs floating noise in mm-scale geometry.
                        "<" => Val::Bool(x < y - 1e-9),
                        "<=" => Val::Bool(x <= y + 1e-9),
                        ">" => Val::Bool(x > y + 1e-9),
                        ">=" => Val::Bool(x >= y - 1e-9),
                        _ => unreachable!("parser only emits known operators"),
                    }
                }
            },
            Expr::Call(f, args) => {
                let xs = args.iter().map(|a| num(a.eval(scope)?)).collect::<Result<Vec<_>, _>>()?;
                match (f.as_str(), xs.as_slice()) {
                    ("abs", [x]) => Val::Num(x.abs()),
                    ("min", [_, ..]) => Val::Num(xs.iter().copied().fold(f64::INFINITY, f64::min)),
                    ("max", [_, ..]) => Val::Num(xs.iter().copied().fold(f64::NEG_INFINITY, f64::max)),
                    _ => return Err(ExprError::Type(format!("bad arguments to {f}()"))),
                }
            }
        })
    }

    /// Every `param.*` name the expression references.
    pub fn params(&self, out: &mut Vec<String>) {
        match self {
            Expr::Name(n) => {
                if let Some(p) = n.strip_prefix("param.") {
                    out.push(p.to_string());
                }
            }
            Expr::Unary(_, e) => e.params(out),
            Expr::Binary(_, a, b) => {
                a.params(out);
                b.params(out);
            }
            Expr::Call(_, args) => args.iter().for_each(|a| a.params(out)),
            Expr::Lit(_) => {}
        }
    }
}
