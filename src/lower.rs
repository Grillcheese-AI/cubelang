//! Fix list P6-3 and P6-4 (2026-11-13): conditions with `&&`, `||`, `!` and `else if`, and expressions as operands.
//!
//! An instruction's operand is a register, a literal or a field read: nothing that needs an instruction of its own.
//! Until P6-4 any other expression in an operand position (`return a + b * 2`, `if (xs.len() > 3)`, `f(s[0])`) was a
//! compile error (DRIFT.md C11). `lower` now computes such an expression into a fresh register first -- the
//! instructions are emitted just before the one that reads it -- and hands back that register as the operand.
//!
//! A condition is lowered by `branch`, which jumps to a label when the condition is false and falls through when it is
//! true. `&&` and `||` short-circuit: the right side's instructions run only when the left side did not decide, so
//! `i < n && s[i] == "x"` never indexes past the end. A condition's instructions are emitted where the condition is
//! tested, so a `while` recomputes them on every turn.
//!
//! Values: a comparison, `&&`, `||` or `!` used as a value is 1 or 0 (as `true` and `false` compile). `a // b` is
//! floor division and `a % b` its remainder (P6-5: DIV with a third operand, 1 or 2; see `engine.rs`). A method call
//! `x.m(args)` calls `m` with `x` first (the string helpers of P6-7); `x.len()` is LEN.
use super::*;

/// Can `e` be written straight into an operand, with no instruction to compute it?
pub(super) fn is_operand(e: &Expr) -> bool {
    match e {
        Expr::IntLit(_) | Expr::FloatLit(_) | Expr::StringLit(_) | Expr::BoolLit(_) | Expr::Null
        | Expr::Ident(_) | Expr::SelfAccess(_) => true,
        Expr::Field(..) => field_path(e).is_some(),
        _ => false,
    }
}

/// DIV's third operand: 1 = whole-number (floor) division, 2 = its remainder (P6-5). Absent: `div` as before.
pub(super) const DIV_FLOOR: i64 = 1;
pub(super) const DIV_REM: i64 = 2;

impl Compiler {
    /// `e` as an operand: a compound expression is computed into a fresh register (its instructions emitted now) and
    /// that register is returned. Forms that cannot be computed (a lambda, `await`, ...) come back unchanged, and
    /// `compile_expr_operand` reports them as before.
    pub(super) fn lower(&mut self, e: &Expr) -> Expr {
        if is_operand(e) {
            return e.clone();
        }
        match e {
            Expr::BinOp(l, bop, r) => {
                let (code, mode) = match bop {
                    BinOp::Add => (op::ADD, None),
                    BinOp::Sub => (op::SUB, None),
                    BinOp::Mul => (op::MUL, None),
                    BinOp::Div => (op::DIV, None),
                    BinOp::IntDiv => (op::DIV, Some(DIV_FLOOR)),
                    BinOp::Mod => (op::DIV, Some(DIV_REM)),
                    _ => return self.lower_bool(e),
                };
                let lo = self.lower(l);
                let ro = self.lower(r);
                let t = self.fresh_temp("expr");
                self.emit_ins(op::ASSIGN, "expr", &t, &[lo]);
                match mode {
                    Some(m) => {
                        // the program is written as .cubebin v6, which an older reader refuses (P6-5)
                        self.whole_div = true;
                        self.emit_ins(code, "expr", &t, &[ro, Expr::IntLit(m)])
                    }
                    None => self.emit_ins(code, "expr", &t, &[ro]),
                }
                Expr::Ident(t)
            }
            Expr::UnaryOp(UnaryOp::Not, _) => self.lower_bool(e),
            Expr::UnaryOp(UnaryOp::Neg, x) => {
                let xo = self.lower(x);
                let t = self.fresh_temp("neg");
                self.emit_ins(op::ASSIGN, "neg", &t, &[Expr::IntLit(0)]);
                self.emit_ins(op::SUB, "neg", &t, &[xo]);
                Expr::Ident(t)
            }
            Expr::Call(callee, args) => {
                self.emit_call(callee, args);
                self.pop_into("call")
            }
            Expr::MethodCall(obj, method, args) => {
                if let Some(routed) = self.route_call(e) {
                    return self.lower(&routed);
                }
                if method == "len" && args.is_empty() {
                    let o = self.to_register(obj);
                    let t = self.fresh_temp("len");
                    self.emit_ins(op::LEN, "len", &t, &[o]);
                    return Expr::Ident(t);
                }
                let mut operands = vec![self.lower(obj)];
                operands.extend(args.iter().map(|a| self.lower(a)));
                self.emit_debug(op::CALL, &format!("call .{}({})", method, args.len()));
                self.emit_byte(op::CALL);
                self.emit_count(operands.len() + 1, "a call's arguments");
                self.emit_global(method);
                for o in &operands {
                    self.compile_expr_operand(o);
                }
                self.pop_into("call")
            }
            Expr::Index(base, idx) => {
                let b = self.to_register(base);
                let i = self.lower(idx);
                let t = self.fresh_temp("index");
                self.emit_ins(op::INDEX, "index", &t, &[b, i]);
                Expr::Ident(t)
            }
            Expr::ArrayLit(xs) => {
                let xs: Vec<Expr> = xs.iter().map(|x| self.lower(x)).collect();
                let t = self.fresh_temp("array");
                self.emit_byte(op::MAKE_ARRAY);
                self.emit_count(xs.len() + 2, "a list literal");
                self.emit_named(&t);
                self.emit_imm(xs.len() as i64);
                for x in &xs {
                    self.compile_expr_operand(x);
                }
                Expr::Ident(t)
            }
            Expr::StructLit(name, fields) => {
                let fields = fields.iter().map(|(k, v)| (k.clone(), self.lower(v))).collect();
                self.materialize(&Expr::StructLit(name.clone(), fields))
            }
            Expr::MapLit(pairs) => {
                let pairs = pairs.iter().map(|(k, v)| (self.lower(k), self.lower(v))).collect();
                self.materialize(&Expr::MapLit(pairs))
            }
            // `f().x`, `xs[0].name`: the base computed first, then the field read from it
            Expr::Field(base, name) => {
                let b = self.to_register(base);
                Expr::Field(Box::new(b), name.clone())
            }
            _ => e.clone(),
        }
    }

    /// `e` in a register (an identifier or a storage field): what INDEX, LEN and a field read take as their container.
    pub(super) fn to_register(&mut self, e: &Expr) -> Expr {
        let o = self.lower(e);
        if matches!(o, Expr::Ident(_) | Expr::SelfAccess(_)) {
            return o;
        }
        let t = self.fresh_temp("reg");
        self.emit_ins(op::ASSIGN, "reg", &t, &[o]);
        Expr::Ident(t)
    }

    /// A condition as a value: 1 when it holds, 0 when not.
    fn lower_bool(&mut self, e: &Expr) -> Expr {
        let t = self.fresh_temp("bool");
        let done = self.fresh_label("bool");
        self.emit_ins(op::ASSIGN, "bool", &t, &[Expr::IntLit(0)]);
        self.branch(e, &done);
        self.emit_ins(op::ASSIGN, "bool", &t, &[Expr::IntLit(1)]);
        self.emit_label(&done);
        Expr::Ident(t)
    }

    /// Jump to `on_false` when `cond` is false; fall through when it is true. `&&` and `||` short-circuit.
    pub(super) fn branch(&mut self, cond: &Expr, on_false: &str) {
        match cond {
            Expr::BinOp(a, BinOp::And, b) => {
                self.branch(a, on_false);
                self.branch(b, on_false);
            }
            Expr::BinOp(a, BinOp::Or, b) => {
                let right = self.fresh_label("or_right");
                let holds = self.fresh_label("or_holds");
                self.branch(a, &right);
                self.emit_jmp(&holds);
                self.emit_label(&right);
                self.branch(b, on_false);
                self.emit_label(&holds);
            }
            Expr::UnaryOp(UnaryOp::Not, a) => {
                let holds = self.fresh_label("not");
                self.branch(a, &holds);
                self.emit_jmp(on_false);
                self.emit_label(&holds);
            }
            _ => {
                self.compile_condition(cond);
                self.emit_cond_jump(on_false);
            }
        }
    }

    /// The statement with its compound operands computed into registers first (their instructions emitted now), or
    /// `None` when it has none. Conditions are lowered where they are tested (`branch`), not here.
    pub(super) fn lower_stmt(&mut self, stmt: &Stmt) -> Option<Stmt> {
        match stmt {
            Stmt::Let(l) => {
                let value = match &l.value {
                    // built in place by compile_let / emit_call, with its arguments computed first
                    Expr::Call(c, args) => Expr::Call(c.clone(), self.lower_args(args)?),
                    Expr::MethodCall(obj, m, args) if m == "len" && args.is_empty()
                        && matches!(obj.as_ref(), Expr::Ident(_)) => return None,
                    Expr::StructLit(n, fs) if fs.iter().any(|(_, v)| !is_operand(v)) => {
                        Expr::StructLit(n.clone(), fs.iter().map(|(k, v)| (k.clone(), self.lower(v))).collect())
                    }
                    Expr::MapLit(ps) if ps.iter().any(|(k, v)| !is_operand(k) || !is_operand(v)) => {
                        Expr::MapLit(ps.iter().map(|(k, v)| (self.lower(k), self.lower(v))).collect())
                    }
                    Expr::StructLit(..) | Expr::MapLit(..) => return None,
                    Expr::ArrayLit(xs) if xs.iter().all(is_operand) => return None,
                    Expr::ArrayLit(xs) => Expr::ArrayLit(xs.iter().map(|x| self.lower(x)).collect()),
                    Expr::Index(b, i) if matches!(b.as_ref(), Expr::Ident(_) | Expr::SelfAccess(_)) && is_operand(i) => {
                        return None
                    }
                    Expr::Index(b, i) => Expr::Index(Box::new(self.to_register(b)), Box::new(self.lower(i))),
                    v if is_operand(v) => return None,
                    v => self.lower(v),
                };
                Some(Stmt::Let(LetStmt { value, ..l.clone() }))
            }
            Stmt::Assign(a) => {
                let compound_key = matches!(&a.target, Expr::Index(_, k) if !is_operand(k));
                if is_operand(&a.value) && !compound_key {
                    return None;
                }
                let target = match &a.target {
                    Expr::Index(b, k) if compound_key => Expr::Index(b.clone(), Box::new(self.lower(k))),
                    t => t.clone(),
                };
                let value = self.lower(&a.value);
                Some(Stmt::Assign(AssignStmt { target, value, ..a.clone() }))
            }
            Stmt::Return(Some(Expr::Call(c, args))) => {
                Some(Stmt::Return(Some(Expr::Call(c.clone(), self.lower_args(args)?))))
            }
            Stmt::Return(Some(e)) if !is_operand(e) => Some(Stmt::Return(Some(self.lower(e)))),
            Stmt::Expr(Expr::Call(c, args)) => Some(Stmt::Expr(Expr::Call(c.clone(), self.lower_args(args)?))),
            Stmt::Expr(Expr::MethodCall(o, m, args)) => {
                Some(Stmt::Expr(Expr::MethodCall(o.clone(), m.clone(), self.lower_args(args)?)))
            }
            Stmt::Opcode(o) => {
                use OpcodeStmt::*;
                let (reg, value) = match o {
                    Assign { reg, value } | Add { reg, value } | Sub { reg, value } | Mul { reg, value }
                    | Div { reg, value } if !is_operand(value) => (reg.clone(), self.lower(value)),
                    _ => return None,
                };
                Some(Stmt::Opcode(match o {
                    Assign { .. } => Assign { reg, value },
                    Add { .. } => Add { reg, value },
                    Sub { .. } => Sub { reg, value },
                    Mul { .. } => Mul { reg, value },
                    _ => Div { reg, value },
                }))
            }
            _ => None,
        }
    }

    /// A call's arguments with the compound ones computed into registers, or `None` when every one is an operand.
    fn lower_args(&mut self, args: &[Expr]) -> Option<Vec<Expr>> {
        if args.iter().all(is_operand) {
            return None;
        }
        Some(args.iter().map(|a| self.lower(a)).collect())
    }

    /// One instruction `code dst, operands...`.
    fn emit_ins(&mut self, code: u8, what: &str, dst: &str, operands: &[Expr]) {
        self.emit_debug(code, &format!("{} {}", what, dst));
        self.emit_byte(code);
        self.emit_count(operands.len() + 1, "an instruction's operands");
        self.emit_named(dst);
        for o in operands {
            self.compile_expr_operand(o);
        }
    }

    /// POP the value a CALL left on the stack into a fresh register.
    fn pop_into(&mut self, what: &str) -> Expr {
        let t = self.fresh_temp(what);
        self.emit_debug(op::POP, &format!("pop {}", t));
        self.emit_byte(op::POP);
        self.emit_byte(1);
        self.emit_named(&t);
        Expr::Ident(t)
    }
}
