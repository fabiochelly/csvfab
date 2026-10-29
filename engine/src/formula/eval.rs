//! The compiled form of a formula — a tree whose columns are resolved to
//! slots, constants folded, text patterns compiled and hot calls specialised —
//! and its evaluation on one row. A cell is borrowed, not copied, through the
//! whole evaluation: `num({Amount}) > 1000 && contains({City}, "lyon")`
//! allocates nothing per row.

use super::fx::{self, Fx, Meth, Mf, Nat};
use super::jsre::{self, Rx};
use super::value::*;
use memchr::memmem::Finder;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bop {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Pow,
    Lt,
    Gt,
    Le,
    Ge,
    Eq,
    Ne,
    StrictEq,
    StrictNe,
}

impl Bop {
    pub fn by_name(op: &str) -> Option<Bop> {
        Some(match op {
            "+" => Bop::Add,
            "-" => Bop::Sub,
            "*" => Bop::Mul,
            "/" => Bop::Div,
            "%" => Bop::Rem,
            "**" => Bop::Pow,
            "<" => Bop::Lt,
            ">" => Bop::Gt,
            "<=" => Bop::Le,
            ">=" => Bop::Ge,
            "==" => Bop::Eq,
            "!=" => Bop::Ne,
            "===" => Bop::StrictEq,
            "!==" => Bop::StrictNe,
            _ => return None,
        })
    }
}

pub enum Node {
    Const(Val<'static>),
    /// A column's cell, by its slot among the columns the formula reads.
    Col(usize),
    /// The row's number (1 for the first data row).
    Row,
    /// The value being cleaned (the duplicates' key formulas).
    V,
    Re(Box<Rx>),
    List(Vec<Node>),
    Not(Box<Node>),
    Neg(Box<Node>),
    Pos(Box<Node>),
    Bin(Bop, Box<(Node, Node)>),
    And(Box<(Node, Node)>),
    Or(Box<(Node, Node)>),
    Nullish(Box<(Node, Node)>),
    Cond(Box<(Node, Node, Node)>),
    Index(Box<(Node, Node)>),
    Fx(Fx, Vec<Node>),
    Math(Mf, Vec<Node>),
    /// receiver, method, arguments, the receiver's source (for messages).
    Method(Box<Node>, Meth, Vec<Node>, Box<str>),
    Length(Box<Node>),
    /// contains(x, "text"): the needle folded once, searched with memchr.
    Contains(Box<Node>, Box<Finder<'static>>),
}

/// What a row gives the formula.
pub struct Cx<'a> {
    /// The cells of the columns read, by slot.
    pub cells: &'a [String],
    pub row: f64,
    pub v: &'a str,
    pub comma: bool,
    pub today: f64,
}

/// Per-thread buffers: never borrowed by a value, reused from row to row.
#[derive(Default)]
pub struct Scratch {
    pub fold: String,
    pub fold2: String,
    regexes: HashMap<(Box<str>, &'static str), Built>,
}

type Built = Result<Arc<Rx>, String>;

impl Scratch {
    /// A pattern built from a text at run time (a cell, a computed value): compiled once per thread.
    pub fn regex(&mut self, src: &str, flags: &'static str) -> Result<Arc<Rx>, String> {
        if let Some(r) = self.regexes.get(&(src.into(), flags)) {
            return r.clone();
        }
        if self.regexes.len() > 256 {
            self.regexes.clear();
        }
        let r = jsre::build(src, flags).map(Arc::new);
        self.regexes.insert((src.into(), flags), r.clone());
        r
    }
}

/// Arguments of a call, on the stack up to four.
pub struct Args<'a>(ArgVec<'a>);

enum ArgVec<'a> {
    Small(usize, [Val<'a>; 4]),
    Big(Vec<Val<'a>>),
}

impl<'a> Args<'a> {
    pub fn len(&self) -> usize {
        match &self.0 { ArgVec::Small(n, _) => *n, ArgVec::Big(v) => v.len() }
    }

    pub fn slice(&self) -> &[Val<'a>] {
        match &self.0 { ArgVec::Small(n, a) => &a[..*n], ArgVec::Big(v) => v }
    }

    /// Argument i, undefined when not given.
    pub fn get(&self, i: usize) -> &Val<'a> {
        self.slice().get(i).unwrap_or(&UNDEF)
    }

    pub fn take(&mut self, i: usize) -> Val<'a> {
        let s = match &mut self.0 { ArgVec::Small(n, a) => &mut a[..*n], ArgVec::Big(v) => &mut v[..] };
        s.get_mut(i).map_or(Val::Undef, |x| std::mem::replace(x, Val::Undef))
    }

    /// The method form: the value becomes the helper's first argument.
    pub fn prepend(self, first: Val<'a>) -> Args<'a> {
        match self.0 {
            ArgVec::Small(n, a) if n < 4 => {
                let [x, y, z, _] = a;
                Args(ArgVec::Small(n + 1, [first, x, y, z]))
            }
            ArgVec::Small(n, a) => {
                let mut v = Vec::with_capacity(n + 1);
                v.push(first);
                v.extend(a);
                Args(ArgVec::Big(v))
            }
            ArgVec::Big(mut v) => {
                v.insert(0, first);
                Args(ArgVec::Big(v))
            }
        }
    }
}

fn args<'a>(nodes: &'a [Node], cx: &Cx<'a>, sc: &mut Scratch) -> Result<Args<'a>, String> {
    if nodes.len() <= 4 {
        let mut a = [Val::Undef, Val::Undef, Val::Undef, Val::Undef];
        for (k, n) in nodes.iter().enumerate() {
            a[k] = eval(n, cx, sc)?;
        }
        return Ok(Args(ArgVec::Small(nodes.len(), a)));
    }
    Ok(Args(ArgVec::Big(nodes.iter().map(|n| eval(n, cx, sc)).collect::<Result<_, _>>()?)))
}

pub fn eval<'a>(n: &'a Node, cx: &Cx<'a>, sc: &mut Scratch) -> R<'a> {
    Ok(match n {
        Node::Const(v) => v.reborrow(),
        Node::Col(s) => Val::str(&cx.cells[*s]),
        Node::Row => Val::Num(cx.row),
        Node::V => Val::str(cx.v),
        Node::Re(r) => Val::Re(r),
        Node::List(items) => Val::List(items.iter().map(|n| eval(n, cx, sc)).collect::<Result<_, _>>()?),
        Node::Not(x) => Val::Bool(!truthy(&eval(x, cx, sc)?)),
        Node::Neg(x) => Val::Num(-to_num(&eval(x, cx, sc)?)),
        Node::Pos(x) => Val::Num(to_num(&eval(x, cx, sc)?)),
        Node::Bin(op, ab) => {
            let a = eval(&ab.0, cx, sc)?;
            let b = eval(&ab.1, cx, sc)?;
            binary(*op, a, b)
        }
        Node::And(ab) => {
            let a = eval(&ab.0, cx, sc)?;
            if truthy(&a) { eval(&ab.1, cx, sc)? } else { a }
        }
        Node::Or(ab) => {
            let a = eval(&ab.0, cx, sc)?;
            if truthy(&a) { a } else { eval(&ab.1, cx, sc)? }
        }
        Node::Nullish(ab) => {
            let a = eval(&ab.0, cx, sc)?;
            if a.nullish() { eval(&ab.1, cx, sc)? } else { a }
        }
        Node::Cond(abc) => {
            if truthy(&eval(&abc.0, cx, sc)?) { eval(&abc.1, cx, sc)? } else { eval(&abc.2, cx, sc)? }
        }
        Node::Index(ab) => {
            let x = eval(&ab.0, cx, sc)?;
            let i = eval(&ab.1, cx, sc)?;
            fx::at(x, &i)
        }
        Node::Fx(f, a) => {
            let a = args(a, cx, sc)?;
            fx::call(*f, a, cx, sc)?
        }
        Node::Math(f, a) => {
            let a = args(a, cx, sc)?;
            Val::Num(fx::math(*f, &a))
        }
        Node::Method(recv, m, a, text) => {
            let r = eval(recv, cx, sc)?;
            let a = args(a, cx, sc)?;
            fx::method(r, *m, a, text, cx, sc)?
        }
        Node::Length(x) => fx::length(&eval(x, cx, sc)?)?,
        Node::Contains(x, needle) => {
            let v = eval(x, cx, sc)?;
            let s = cell_str(&v);
            sc.fold.clear();
            crate::text::fold_into(&s, &mut sc.fold);
            Val::Bool(needle.find(sc.fold.as_bytes()).is_some())
        }
    })
}

pub fn binary<'a>(op: Bop, a: Val<'a>, b: Val<'a>) -> Val<'a> {
    let num = |f: fn(f64, f64) -> f64, a: &Val, b: &Val| Val::Num(f(to_num(a), to_num(b)));
    match op {
        Bop::Add => match (a, b) {
            (Val::Num(x), Val::Num(y)) => Val::Num(x + y),
            (a, b) => {
                let (a, b) = (to_prim(a, false), to_prim(b, false));
                if !matches!(a, Val::Str(_)) && !matches!(b, Val::Str(_)) {
                    return Val::Num(to_num(&a) + to_num(&b));
                }
                // Grows a computed text in place: "a" + {B} + "c" copies each part once.
                let mut s = match a {
                    Val::Str(s) => s.into_owned(),
                    a => to_str(&a).into_owned(),
                };
                write_str(&b, &mut s);
                Val::owned(s)
            }
        },
        Bop::Sub => num(|x, y| x - y, &a, &b),
        Bop::Mul => num(|x, y| x * y, &a, &b),
        Bop::Div => num(|x, y| x / y, &a, &b),
        Bop::Rem => num(|x, y| x % y, &a, &b),
        Bop::Pow => num(fx::pow, &a, &b),
        Bop::Lt => Val::Bool(compare(a, b).is_some_and(|o| o.is_lt())),
        Bop::Gt => Val::Bool(compare(a, b).is_some_and(|o| o.is_gt())),
        Bop::Le => Val::Bool(compare(a, b).is_some_and(|o| o.is_le())),
        Bop::Ge => Val::Bool(compare(a, b).is_some_and(|o| o.is_ge())),
        Bop::Eq => Val::Bool(loose_eq(a, b)),
        Bop::Ne => Val::Bool(!loose_eq(a, b)),
        Bop::StrictEq => Val::Bool(strict_eq(&a, &b)),
        Bop::StrictNe => Val::Bool(!strict_eq(&a, &b)),
    }
}

// ---- compile-time optimisation ---------------------------------------------------

/// Whether the node always gives a text: its helper methods can then be called directly.
fn gives_text(n: &Node) -> bool {
    match n {
        Node::Col(_) | Node::V => true,
        Node::Const(Val::Str(_)) => true,
        Node::Fx(f, _) => f.gives_text(),
        Node::Method(_, Meth::Nat(n), _, _) => matches!(n, Nat::Slice | Nat::Substring | Nat::ToLowerCase | Nat::ToUpperCase
            | Nat::ToLocaleLowerCase | Nat::ToLocaleUpperCase | Nat::Trim | Nat::TrimStart | Nat::TrimEnd | Nat::PadEnd | Nat::PadStart
            | Nat::Repeat | Nat::Concat | Nat::Normalize),
        _ => false,
    }
}

fn foldable(n: &Node) -> bool {
    let all = |v: &[Node]| v.iter().all(|x| matches!(x, Node::Const(_)));
    match n {
        Node::Not(x) | Node::Neg(x) | Node::Pos(x) | Node::Length(x) => matches!(**x, Node::Const(_)),
        Node::Bin(_, ab) | Node::Index(ab) | Node::Nullish(ab) => matches!((&ab.0, &ab.1), (Node::Const(_), Node::Const(_))),
        Node::Fx(f, a) => f.pure() && all(a),
        Node::Math(f, a) => *f != Mf::Random && all(a),
        Node::Method(r, _, a, _) => matches!(**r, Node::Const(_)) && all(a),
        _ => false,
    }
}

/// Folds what does not depend on the row, compiles the patterns given as
/// text, and specialises hot calls. Err: a pattern the guard refuses.
pub fn optimize(n: Node, comma: bool) -> Result<Node, String> {
    let opt = |n: Node| optimize(n, comma);
    let opt_all = |v: Vec<Node>| v.into_iter().map(|n| optimize(n, comma)).collect::<Result<Vec<_>, _>>();
    let n = match n {
        Node::List(v) => Node::List(opt_all(v)?),
        Node::Not(x) => Node::Not(Box::new(opt(*x)?)),
        Node::Neg(x) => Node::Neg(Box::new(opt(*x)?)),
        Node::Pos(x) => Node::Pos(Box::new(opt(*x)?)),
        Node::Length(x) => Node::Length(Box::new(opt(*x)?)),
        Node::Bin(op, ab) => Node::Bin(op, Box::new((opt(ab.0)?, opt(ab.1)?))),
        Node::Index(ab) => Node::Index(Box::new((opt(ab.0)?, opt(ab.1)?))),
        Node::Nullish(ab) => Node::Nullish(Box::new((opt(ab.0)?, opt(ab.1)?))),
        // A constant test picks its branch now.
        Node::And(ab) => match opt(ab.0)? {
            Node::Const(c) if truthy(&c) => opt(ab.1)?,
            Node::Const(c) => Node::Const(c),
            a => Node::And(Box::new((a, opt(ab.1)?))),
        },
        Node::Or(ab) => match opt(ab.0)? {
            Node::Const(c) if truthy(&c) => Node::Const(c),
            Node::Const(_) => opt(ab.1)?,
            a => Node::Or(Box::new((a, opt(ab.1)?))),
        },
        Node::Cond(abc) => match opt(abc.0)? {
            Node::Const(c) => if truthy(&c) { opt(abc.1)? } else { opt(abc.2)? },
            c => Node::Cond(Box::new((c, opt(abc.1)?, opt(abc.2)?))),
        },
        Node::Math(f, a) => Node::Math(f, opt_all(a)?),
        Node::Fx(f, a) => Node::Fx(f, patterns(f.pattern_flags(), 1, opt_all(a)?)?),
        Node::Method(r, m, a, text) => {
            let r = opt(*r)?;
            let flags = match m { Meth::Nat(Nat::Match | Nat::Search) => Some(""), _ => None };
            let a = patterns(flags, 0, opt_all(a)?)?;
            match m {
                // {City}.contains("x") is contains({City}, "x") when {City} is surely a text.
                Meth::Fx(f) if gives_text(&r) => {
                    let mut all = vec![r];
                    all.extend(a);
                    return optimize(Node::Fx(f, all), comma);
                }
                _ => Node::Method(Box::new(r), m, a, text),
            }
        }
        n => n,
    };
    if let Node::Fx(Fx::Contains, a) = &n {
        if let (2, Some(Node::Const(Val::Str(p)))) = (a.len(), a.get(1)) {
            let needle = crate::text::fold(p);
            let Node::Fx(_, mut a) = n else { unreachable!() };
            a.truncate(1);
            return Ok(Node::Contains(Box::new(a.pop().unwrap()), Box::new(Finder::new(needle.as_bytes()).into_owned())));
        }
    }
    if foldable(&n) {
        let cx = Cx { cells: &[], row: f64::NAN, v: "", comma, today: f64::NAN };
        if let Ok(v) = eval(&n, &cx, &mut Scratch::default()) {
            if let Some(v) = v.into_static() {
                return Ok(Node::Const(v));
            }
        }
    }
    Ok(n)
}

/// A pattern given as a constant text becomes a regex now, with the flags its
/// function would give it — the same match, compiled once instead of per row.
/// A text that is no valid pattern is left to fail on each row, as it did.
fn patterns(flags: Option<&'static str>, at: usize, mut a: Vec<Node>) -> Result<Vec<Node>, String> {
    let Some(flags) = flags else { return Ok(a) };
    if let Some(Node::Const(c)) = a.get(at) {
        if !matches!(c, Val::List(_)) {
            let src = if let Val::Undef = c { String::new() } else { to_str(c).into_owned() };
            match jsre::build(&src, flags) {
                Ok(rx) => a[at] = Node::Re(Box::new(rx)),
                Err(e) if e == jsre::TOO_BIG => return Err(e),
                Err(_) => {}
            }
        }
    }
    Ok(a)
}
