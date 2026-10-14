//! Compile-once arithmetic expressions for algebraic (`cc_type = 3`)
//! conversions.
//!
//! Replaces `meval`, which (a) re-parsed the formula text and rebuilt a
//! builtin-function table for **every sample**, and (b) pulled in `nom 1.x`,
//! which future Rust versions reject.
//!
//! Grammar (meval-compatible, plus `log`/`log10`/`pow`):
//!
//! ```text
//! expr   := term (('+' | '-') term)*
//! term   := unary (('*' | '/' | '%') unary)*
//! unary  := ('+' | '-') unary | power
//! power  := atom ('^' unary)?            // right-assoc, binds tighter than unary minus
//! atom   := number | ident | ident '(' args ')' | '(' expr ')'
//! ```
//!
//! The variable is `X` (also `X1`, as written by MDF3→MDF4 converters).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

#[derive(Debug, Clone)]
pub enum Expr {
    Num(f64),
    Var,
    Neg(Box<Expr>),
    Bin(Op, Box<Expr>, Box<Expr>),
    Call(Func, Vec<Expr>),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Pow,
}

#[derive(Debug, Clone, Copy)]
pub enum Func {
    F1(fn(f64) -> f64),
    F2(fn(f64, f64) -> f64),
    /// Variadic min / max (meval semantics: at least one argument).
    Min,
    Max,
}

fn lookup_func(name: &str) -> Option<Func> {
    use Func::*;
    Some(match name {
        "sqrt" => F1(f64::sqrt),
        "exp" => F1(f64::exp),
        "ln" | "log" => F1(f64::ln),
        "log10" => F1(f64::log10),
        "abs" => F1(f64::abs),
        "sin" => F1(f64::sin),
        "cos" => F1(f64::cos),
        "tan" => F1(f64::tan),
        "asin" => F1(f64::asin),
        "acos" => F1(f64::acos),
        "atan" => F1(f64::atan),
        "sinh" => F1(f64::sinh),
        "cosh" => F1(f64::cosh),
        "tanh" => F1(f64::tanh),
        "asinh" => F1(f64::asinh),
        "acosh" => F1(f64::acosh),
        "atanh" => F1(f64::atanh),
        "floor" => F1(f64::floor),
        "ceil" => F1(f64::ceil),
        "round" => F1(f64::round),
        "signum" => F1(f64::signum),
        "atan2" => F2(f64::atan2),
        "pow" => F2(f64::powf),
        "min" => Min,
        "max" => Max,
        _ => return None,
    })
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Ident(String),
    Op(char),
    LParen,
    RParen,
    Comma,
}

fn tokenize(s: &str) -> Result<Vec<Tok>, String> {
    let b = s.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < b.len() {
        let c = b[i] as char;
        if c.is_ascii_whitespace() {
            i += 1;
        } else if c.is_ascii_digit() || (c == '.' && b.get(i + 1).is_some_and(|d| d.is_ascii_digit())) {
            let start = i;
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
                i += 1;
            }
            if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
                let save = i;
                i += 1;
                if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
                    i += 1;
                }
                if i < b.len() && b[i].is_ascii_digit() {
                    while i < b.len() && b[i].is_ascii_digit() {
                        i += 1;
                    }
                } else {
                    i = save;
                }
            }
            let txt = &s[start..i];
            out.push(Tok::Num(txt.parse().map_err(|_| format!("bad number '{txt}'"))?));
        } else if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            out.push(Tok::Ident(s[start..i].to_string()));
        } else {
            out.push(match c {
                '(' => Tok::LParen,
                ')' => Tok::RParen,
                ',' => Tok::Comma,
                '+' | '-' | '*' | '/' | '%' | '^' => Tok::Op(c),
                _ => return Err(format!("unexpected character '{c}'")),
            });
            i += 1;
        }
    }
    Ok(out)
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }
    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        self.pos += 1;
        t
    }
    fn expr(&mut self) -> Result<Expr, String> {
        let mut lhs = self.term()?;
        while let Some(Tok::Op(c @ ('+' | '-'))) = self.peek().cloned() {
            self.pos += 1;
            let rhs = self.term()?;
            lhs = Expr::Bin(if c == '+' { Op::Add } else { Op::Sub }, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }
    fn term(&mut self) -> Result<Expr, String> {
        let mut lhs = self.unary()?;
        while let Some(Tok::Op(c @ ('*' | '/' | '%'))) = self.peek().cloned() {
            self.pos += 1;
            let rhs = self.unary()?;
            let op = match c {
                '*' => Op::Mul,
                '/' => Op::Div,
                _ => Op::Rem,
            };
            lhs = Expr::Bin(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }
    fn unary(&mut self) -> Result<Expr, String> {
        match self.peek() {
            Some(Tok::Op('-')) => {
                self.pos += 1;
                Ok(Expr::Neg(Box::new(self.unary()?)))
            }
            Some(Tok::Op('+')) => {
                self.pos += 1;
                self.unary()
            }
            _ => self.power(),
        }
    }
    fn power(&mut self) -> Result<Expr, String> {
        let base = self.atom()?;
        if let Some(Tok::Op('^')) = self.peek() {
            self.pos += 1;
            let exp = self.unary()?;
            return Ok(Expr::Bin(Op::Pow, Box::new(base), Box::new(exp)));
        }
        Ok(base)
    }
    fn atom(&mut self) -> Result<Expr, String> {
        match self.next() {
            Some(Tok::Num(v)) => Ok(Expr::Num(v)),
            Some(Tok::LParen) => {
                let e = self.expr()?;
                match self.next() {
                    Some(Tok::RParen) => Ok(e),
                    _ => Err("expected ')'".into()),
                }
            }
            Some(Tok::Ident(name)) => {
                if let Some(Tok::LParen) = self.peek() {
                    self.pos += 1;
                    let f = lookup_func(&name).ok_or_else(|| format!("unknown function '{name}'"))?;
                    let mut args = Vec::new();
                    if let Some(Tok::RParen) = self.peek() {
                        self.pos += 1;
                    } else {
                        loop {
                            args.push(self.expr()?);
                            match self.next() {
                                Some(Tok::Comma) => continue,
                                Some(Tok::RParen) => break,
                                _ => return Err("expected ',' or ')'".into()),
                            }
                        }
                    }
                    let ok = match f {
                        Func::F1(_) => args.len() == 1,
                        Func::F2(_) => args.len() == 2,
                        Func::Min | Func::Max => !args.is_empty(),
                    };
                    if !ok {
                        return Err(format!("wrong argument count for '{name}'"));
                    }
                    Ok(Expr::Call(f, args))
                } else {
                    match name.as_str() {
                        "X" | "X1" => Ok(Expr::Var),
                        "pi" => Ok(Expr::Num(std::f64::consts::PI)),
                        "e" => Ok(Expr::Num(std::f64::consts::E)),
                        _ => Err(format!("unknown variable '{name}'")),
                    }
                }
            }
            t => Err(format!("unexpected token {t:?}")),
        }
    }
}

impl Expr {
    /// Parse a formula. Errors on any syntax problem, unknown identifier or
    /// wrong function arity.
    pub fn parse(src: &str) -> Result<Expr, String> {
        let mut p = Parser { toks: tokenize(src)?, pos: 0 };
        let e = p.expr()?;
        if p.pos != p.toks.len() {
            return Err(format!("trailing input at token {}", p.pos));
        }
        Ok(e.fold())
    }

    /// Constant-fold sub-trees that do not depend on `X`.
    fn fold(self) -> Expr {
        match self {
            Expr::Neg(a) => match a.fold() {
                Expr::Num(v) => Expr::Num(-v),
                a => Expr::Neg(Box::new(a)),
            },
            Expr::Bin(op, a, b) => {
                let (a, b) = (a.fold(), b.fold());
                if let (Expr::Num(_), Expr::Num(_)) = (&a, &b) {
                    Expr::Num(Expr::Bin(op, Box::new(a), Box::new(b)).eval(0.0))
                } else {
                    Expr::Bin(op, Box::new(a), Box::new(b))
                }
            }
            Expr::Call(f, args) => {
                let args: Vec<Expr> = args.into_iter().map(Expr::fold).collect();
                if args.iter().all(|a| matches!(a, Expr::Num(_))) {
                    Expr::Num(Expr::Call(f, args).eval(0.0))
                } else {
                    Expr::Call(f, args)
                }
            }
            e => e,
        }
    }

    pub fn eval(&self, x: f64) -> f64 {
        match self {
            Expr::Num(v) => *v,
            Expr::Var => x,
            Expr::Neg(a) => -a.eval(x),
            Expr::Bin(op, a, b) => {
                let (a, b) = (a.eval(x), b.eval(x));
                match op {
                    Op::Add => a + b,
                    Op::Sub => a - b,
                    Op::Mul => a * b,
                    Op::Div => a / b,
                    Op::Rem => a % b,
                    Op::Pow => a.powf(b),
                }
            }
            Expr::Call(f, args) => match f {
                Func::F1(g) => g(args[0].eval(x)),
                Func::F2(g) => g(args[0].eval(x), args[1].eval(x)),
                Func::Min => args.iter().map(|a| a.eval(x)).fold(f64::INFINITY, f64::min),
                Func::Max => args.iter().map(|a| a.eval(x)).fold(f64::NEG_INFINITY, f64::max),
            },
        }
    }
}

type Compiled = Option<Rc<Expr>>;

thread_local! {
    /// Last formula used on this thread (the per-sample hot path).
    static LAST: RefCell<Option<(String, Compiled)>> = const { RefCell::new(None) };
    /// All formulas seen on this thread.
    static CACHE: RefCell<HashMap<String, Compiled>> = RefCell::new(HashMap::new());
}

/// Evaluate `formula` at `x`, compiling it at most once per thread.
/// Returns `None` if the formula does not parse.
pub fn eval_cached(formula: &str, x: f64) -> Option<f64> {
    let hit = LAST.with(|l| match &*l.borrow() {
        Some((f, c)) if f == formula => Some(c.clone()),
        _ => None,
    });
    let compiled = match hit {
        Some(c) => c,
        None => {
            let c = CACHE.with(|cache| {
                let mut cache = cache.borrow_mut();
                if cache.len() > 4096 {
                    cache.clear();
                }
                cache
                    .entry(formula.to_string())
                    .or_insert_with(|| Expr::parse(formula).ok().map(Rc::new))
                    .clone()
            });
            LAST.with(|l| *l.borrow_mut() = Some((formula.to_string(), c.clone())));
            c
        }
    };
    compiled.map(|e| e.eval(x))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(s: &str, x: f64) -> f64 {
        Expr::parse(s).unwrap().eval(x)
    }

    #[test]
    fn precedence_and_associativity() {
        assert_eq!(ev("1 + 2 * 3", 0.0), 7.0);
        assert_eq!(ev("(1 + 2) * 3", 0.0), 9.0);
        assert_eq!(ev("-2^2", 0.0), -4.0);
        assert_eq!(ev("2^3^2", 0.0), 512.0);
        assert_eq!(ev("2^-1", 0.0), 0.5);
        assert_eq!(ev("10 - 4 - 3", 0.0), 3.0);
        assert_eq!(ev("64 / 4 / 2", 0.0), 8.0);
        assert_eq!(ev("7 % 4", 0.0), 3.0);
        assert_eq!(ev("--X", 3.0), 3.0);
    }

    #[test]
    fn variables_numbers_functions() {
        assert_eq!(ev("X * 0.5 + 1.25e1", 4.0), 14.5);
        assert_eq!(ev("X1 * 2", 4.0), 8.0);
        assert_eq!(ev(".5 * X", 4.0), 2.0);
        assert_eq!(ev("sqrt(X) + abs(-1)", 16.0), 5.0);
        assert_eq!(ev("max(1, X, 3)", 7.0), 7.0);
        assert_eq!(ev("min(X)", 7.0), 7.0);
        assert!((ev("sin(pi / 2)", 0.0) - 1.0).abs() < 1e-15);
        assert!((ev("ln(e)", 0.0) - 1.0).abs() < 1e-15);
        assert_eq!(ev("log10(1000)", 0.0), 3.0);
        assert_eq!(ev("pow(2, X)", 10.0), 1024.0);
        assert_eq!(ev("atan2(0, 1)", 0.0), 0.0);
    }

    #[test]
    fn errors_are_reported() {
        for bad in ["", "X +", "foo(1)", "Y * 2", "sqrt(1, 2)", "(X", "X)", "1 $ 2", "X10", "min()"] {
            assert!(Expr::parse(bad).is_err(), "{bad:?} should fail");
        }
    }

    #[test]
    fn constant_folding_keeps_semantics() {
        assert!(matches!(Expr::parse("2 * 3 + 1").unwrap(), Expr::Num(v) if v == 7.0));
        assert_eq!(ev("X * (2 * 3)", 2.0), 12.0);
        assert!(ev("1 / 0", 0.0).is_infinite());
    }

    #[test]
    fn cache_hits_and_misses() {
        assert_eq!(eval_cached("X + 1", 1.0), Some(2.0));
        assert_eq!(eval_cached("X + 1", 2.0), Some(3.0));
        assert_eq!(eval_cached("X * 10", 2.0), Some(20.0));
        assert_eq!(eval_cached("X + 1", 5.0), Some(6.0));
        assert_eq!(eval_cached("nonsense(", 5.0), None);
    }
}
