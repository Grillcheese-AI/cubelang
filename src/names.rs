//! Names a function reads as values but never declares (fix list P6-1, strict mode).
//!
//! A register is addressed by a hash of its name, so a misspelled name used to read as a register nothing ever wrote:
//! null, which then compared equal to any string. Under strict compilation (the default) such a read is now a compile
//! error. A name counts as declared when it is a parameter, a `let`, a `for` binding, a `catch` binding, a register an
//! opcode statement or an `asm` block writes or names (`create`, `assign`, `pop`, `add`, ...), an assignment's target,
//! or a name the file declares (a storage field, a function, a program or container, a struct, an enum, a type alias,
//! a `use`d module), given by the compiler as `known`. Declarations are gathered over the whole function body first, so
//! the check is about names that are never declared anywhere, not about order.
//!
//! The VM's positional registers (`arg0`, `arg1`, ...) and a bare `recall key;`'s register count as declared.
//!
//! Only value positions are checked: conditions, right-hand sides, returns, call arguments, indexes, `for` iterables.
//! Positions where a bare word is a symbol, not a register (a role or filler of `bind`, a key of `store`/`recall`, an
//! extended opcode's arguments, a map literal's bare keys, a callee's name, a match pattern, the bare-word arguments of a
//! `use`d module's function such as `vsa.recover(frame, SUBJECT)`, where a role is a symbol) are not.
use std::collections::HashSet;

use crate::ast::{AsmOperand, Expr, ExtArg, FunctionDecl, OpcodeStmt, Stmt};

/// The names `func` reads as values and never declares, sorted, each once.
/// `symbolic(name)`: is `name` a `use`d module's function, whose bare-word arguments are symbols?
pub fn undeclared(func: &FunctionDecl, known: &HashSet<String>, symbolic: &dyn Fn(&str) -> bool) -> Vec<String> {
    let mut declared: HashSet<String> = known.clone();
    for p in &func.sig.params {
        declared.insert(p.name.clone());
    }
    declare(&func.body, &mut declared);
    let mut out = Vec::new();
    let cx = Cx { d: &declared, symbolic };
    for s in &func.body {
        check_stmt(s, &cx, &mut out);
    }
    out.sort();
    out.dedup();
    out
}

struct Cx<'a> {
    d: &'a HashSet<String>,
    symbolic: &'a dyn Fn(&str) -> bool,
}

/// `arg0`, `arg1`, ...: the registers a call's arguments arrive in when the callee names none.
fn positional(n: &str) -> bool {
    n.len() > 3 && n.starts_with("arg") && n[3..].bytes().all(|b| b.is_ascii_digit())
}

fn declare(body: &[Stmt], d: &mut HashSet<String>) {
    for s in body {
        match s {
            Stmt::Let(l) => { d.insert(l.name.clone()); }
            Stmt::Assign(a) => {
                if let Expr::Ident(n) = &a.target { d.insert(n.clone()); }
            }
            Stmt::If(i) => {
                declare(&i.then_body, d);
                if let Some(e) = &i.else_body { declare(e, d); }
            }
            Stmt::For(f) => { d.insert(f.binding.clone()); declare(&f.body, d); }
            Stmt::While(w) => declare(&w.body, d),
            Stmt::Match(m) => for arm in &m.arms { declare(&arm.body, d) },
            Stmt::TryCatch(t) => {
                declare(&t.try_body, d);
                if let Some(b) = &t.catch_binding { d.insert(b.clone()); }
                declare(&t.catch_body, d);
                if let Some(f) = &t.finally_body { declare(f, d); }
            }
            Stmt::Atomic(a) => declare(&a.body, d),
            Stmt::Opcode(o) => opcode_regs(o, d),
            Stmt::Asm(block) => {
                for i in &block.instrs {
                    for op in &i.operands {
                        if let AsmOperand::Named(n) = op { d.insert(n.clone()); }
                    }
                }
            }
            _ => {}
        }
    }
}

fn opcode_regs(o: &OpcodeStmt, d: &mut HashSet<String>) {
    use OpcodeStmt::*;
    match o {
        Create { reg, .. } | Assign { reg, .. } | Add { reg, .. } | Sub { reg, .. } | Mul { reg, .. }
        | Div { reg, .. } | Sum { reg } | Push { reg } | Pop { reg } | Query { reg } | Remember { reg }
        | Store { reg, .. } | Bind { reg, .. } | BindRole { reg, .. } => { d.insert(reg.clone()); }
        Recall { reg: Some(r), .. } => { d.insert(r.clone()); }
        Recall { reg: None, key } => { if let Expr::Ident(k) = key { d.insert(k.clone()); } }
        Unify { a, b } | Compare { a, b } => { d.insert(a.clone()); d.insert(b.clone()); }
        Transfer { src, dst, .. } => { d.insert(src.clone()); d.insert(dst.clone()); }
        Extended { args, .. } => {
            for a in args {
                if let ExtArg::Reg(n) = a { d.insert(n.clone()); }
            }
        }
    }
}

fn check_body(body: &[Stmt], d: &Cx, out: &mut Vec<String>) {
    for s in body {
        check_stmt(s, d, out);
    }
}

fn check_stmt(s: &Stmt, d: &Cx, out: &mut Vec<String>) {
    match s {
        Stmt::Let(l) => check_expr(&l.value, d, out),
        Stmt::Assign(a) => {
            match &a.target {
                Expr::Ident(_) | Expr::SelfAccess(_) => {}
                Expr::Index(base, key) => { check_expr(base, d, out); check_expr(key, d, out); }
                other => check_expr(other, d, out),
            }
            check_expr(&a.value, d, out);
        }
        Stmt::If(i) => {
            check_expr(&i.condition, d, out);
            check_body(&i.then_body, d, out);
            if let Some(e) = &i.else_body { check_body(e, d, out); }
        }
        Stmt::For(f) => { check_expr(&f.iter, d, out); check_body(&f.body, d, out); }
        Stmt::While(w) => { check_expr(&w.condition, d, out); check_body(&w.body, d, out); }
        Stmt::Match(m) => {
            check_expr(&m.expr, d, out);
            for arm in &m.arms { check_body(&arm.body, d, out); }
        }
        Stmt::Return(Some(e)) | Stmt::Throw(e) | Stmt::Expr(e) => check_expr(e, d, out),
        Stmt::Emit(e) => for (_, v) in &e.fields { check_expr(v, d, out) },
        Stmt::Opcode(o) => match o {
            OpcodeStmt::Assign { value, .. } | OpcodeStmt::Add { value, .. } | OpcodeStmt::Sub { value, .. }
            | OpcodeStmt::Mul { value, .. } | OpcodeStmt::Div { value, .. } => check_expr(value, d, out),
            OpcodeStmt::Transfer { amount, .. } => check_expr(amount, d, out),
            _ => {}
        },
        Stmt::TryCatch(t) => {
            check_body(&t.try_body, d, out);
            check_body(&t.catch_body, d, out);
            if let Some(f) = &t.finally_body { check_body(f, d, out); }
        }
        Stmt::Assert(a) => {
            check_expr(&a.condition, d, out);
            if let Some(m) = &a.message { check_expr(m, d, out); }
        }
        Stmt::Atomic(a) => check_body(&a.body, d, out),
        _ => {}
    }
}

fn check_expr(e: &Expr, d: &Cx, out: &mut Vec<String>) {
    match e {
        Expr::Ident(n) => {
            if n != "_" && !d.d.contains(n) && !positional(n) {
                out.push(n.clone());
            }
        }
        Expr::Field(base, _) | Expr::Await(base) | Expr::UnaryOp(_, base) => check_expr(base, d, out),
        Expr::Index(a, b) | Expr::BinOp(a, _, b) => { check_expr(a, d, out); check_expr(b, d, out); }
        Expr::Call(callee, args) => {
            let symbolic = matches!(callee.as_ref(), Expr::Ident(n) if (d.symbolic)(n));
            if !matches!(callee.as_ref(), Expr::Ident(_)) { check_expr(callee, d, out); }
            for a in args {
                if symbolic && matches!(a, Expr::Ident(_)) { continue; }
                check_expr(a, d, out);
            }
        }
        Expr::MethodCall(obj, _, args) => {
            check_expr(obj, d, out);
            for a in args { check_expr(a, d, out); }
        }
        Expr::StructLit(_, fields) | Expr::Proxy(_, fields) => for (_, v) in fields { check_expr(v, d, out) },
        Expr::ArrayLit(xs) | Expr::Deploy(_, xs) => for x in xs { check_expr(x, d, out) },
        Expr::MapLit(pairs) => {
            for (k, v) in pairs {
                if !matches!(k, Expr::Ident(_)) { check_expr(k, d, out); }
                check_expr(v, d, out);
            }
        }
        _ => {}
    }
}
