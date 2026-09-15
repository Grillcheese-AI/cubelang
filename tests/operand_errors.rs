//! Regression tests for the DRIFT.md §C11 fix.
//!
//! `Compiler::compile_expr_operand` (src/compiler.rs) only has a real
//! encoding for a literal, a bare identifier, `self.field`, and a simple
//! `expr.field`. Every other expression shape used as an operand -- a
//! `BinOp` chief among them -- used to silently fall through to an opaque
//! `OP_NONE` placeholder (the exact bug `docs/DRIFT.md` reproduces with
//! `double_it(x - y)` returning 0 instead of 4, undetected even by
//! `cubelang check`). It is now a hard compile error in EVERY mode -- not
//! just `--strict` -- since this is the one silent-garbage path strict mode
//! itself could never catch (it inspects statement shapes, never descends
//! into an individual operand).
//!
//! One test per path named in the task brief: `emit_call`'s argument loop,
//! array-literal elements, index expressions, and `let`/`assign` RHS.

use cubelang::compiler;

fn assert_binop_operand_error(source: &str, expect_missing_ok: &str) {
    let err = compiler::compile(source).expect_err(expect_missing_ok);
    let msg = err.to_string();
    assert!(msg.contains("BinOp"), "error should name the offending expression form: {}", msg);
    assert!(msg.contains("cannot be encoded as an operand"),
        "error should explain why: {}", msg);
}

// ── (a) call argument: the DRIFT.md reproduction itself ─────────────────────

const CALL_ARGUMENT_BINOP: &str = r#"
program DoubleIt implements ISolve {
    function double_it(n: number): number {
        create r: number;
        assign r = n;
        add r, n;
        return r;
    }

    public function solve(input: str): number {
        create x: number;
        assign x = 5;
        create y: number;
        assign y = 3;
        let z = double_it(x - y);
        return z;
    }
}
"#;

#[test]
fn a_binop_call_argument_is_a_hard_compile_error_in_loose_mode() {
    assert_binop_operand_error(
        CALL_ARGUMENT_BINOP,
        "double_it(x - y) must not compile -- BinOp cannot be encoded as a call argument",
    );
}

#[test]
fn a_binop_call_argument_is_also_rejected_under_strict() {
    // Unconditional: --strict must reject it too (it already did in
    // effect, since compile_ast_strict folds operand_errors in), not just
    // loose `compile()`.
    let err = compiler::compile_strict(CALL_ARGUMENT_BINOP)
        .expect_err("double_it(x - y) must not compile under --strict either");
    assert!(err.contains("BinOp"), "error should name BinOp: {}", err);
}

// ── (b) array-literal element ────────────────────────────────────────────────

const ARRAY_LITERAL_ELEMENT_BINOP: &str = r#"
program ArrLit implements ISolve {
    public function solve(input: str): number {
        create x: number;
        assign x = 5;
        create y: number;
        assign y = 3;
        let xs = [1, x - y, 3];
        return xs[0];
    }
}
"#;

#[test]
fn b_binop_array_literal_element_is_a_hard_compile_error() {
    assert_binop_operand_error(
        ARRAY_LITERAL_ELEMENT_BINOP,
        "[1, x - y, 3] must not compile -- BinOp cannot be encoded as an array element",
    );
}

// ── (c) index expression ─────────────────────────────────────────────────────

const INDEX_EXPRESSION_BINOP: &str = r#"
program IndexExpr implements ISolve {
    public function solve(input: str): number {
        let xs = [10, 20, 30];
        create x: number;
        assign x = 2;
        create y: number;
        assign y = 1;
        let z = xs[x - y];
        return z;
    }
}
"#;

#[test]
fn c_binop_index_expression_is_a_hard_compile_error() {
    assert_binop_operand_error(
        INDEX_EXPRESSION_BINOP,
        "xs[x - y] must not compile -- BinOp cannot be encoded as an index expression",
    );
}

// ── (d) `let` RHS ─────────────────────────────────────────────────────────────

const LET_RHS_BINOP: &str = r#"
program LetRhs implements ISolve {
    public function solve(input: str): number {
        create x: number;
        assign x = 5;
        create y: number;
        assign y = 3;
        let diff = x - y;
        return diff;
    }
}
"#;

#[test]
fn d_binop_let_rhs_is_a_hard_compile_error() {
    assert_binop_operand_error(
        LET_RHS_BINOP,
        "let diff = x - y must not compile -- BinOp cannot be encoded as a let RHS",
    );
}

// ── (e) `assign` RHS ──────────────────────────────────────────────────────────

const ASSIGN_RHS_BINOP: &str = r#"
program AssignRhs implements ISolve {
    public function solve(input: str): number {
        create x: number;
        assign x = 5;
        create y: number;
        assign y = 3;
        create z: number;
        assign z = 0;
        z = x - y;
        return z;
    }
}
"#;

#[test]
fn e_binop_assign_rhs_is_a_hard_compile_error() {
    assert_binop_operand_error(
        ASSIGN_RHS_BINOP,
        "z = x - y must not compile -- BinOp cannot be encoded as an assign RHS",
    );
}

// ── The workaround the error message itself recommends must still work ──────

const WORKAROUND_COMPUTE_INTO_A_REGISTER_FIRST: &str = r#"
program Workaround implements ISolve {
    function double_it(n: number): number {
        create r: number;
        assign r = n;
        add r, n;
        return r;
    }

    public function solve(input: str): number {
        create x: number;
        assign x = 5;
        create y: number;
        assign y = 3;
        create diff: number;
        assign diff = x;
        sub diff, y;
        let z = double_it(diff);
        return z;
    }
}
"#;

#[test]
fn the_recommended_workaround_still_compiles_clean() {
    compiler::compile(WORKAROUND_COMPUTE_INTO_A_REGISTER_FIRST)
        .expect("computing the BinOp into a register first, then passing the register, must still compile");
}
