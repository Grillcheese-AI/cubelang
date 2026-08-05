//! Bugfix: `Engine::resolve_value` (used by RETURN) has no `Operand::Global`
//! arm, so `return "<string literal>";` silently falls through to
//! `Value::Int(0)` instead of the real string -- a string literal compiles to
//! `Operand::Global` via `emit_global`, and `resolve_value`'s match only
//! covers `Imm`/`FloatImm`/`Named`. The fix adds a `Global` arm that resolves
//! via `resolve_symbol_name` -- the same name-table path ASK's question text
//! and BIND_ROLE/UNBIND's role/filler symbols already use.
//!
//! These tests pin the fix: RETURN of a string literal must yield the real
//! `Value::Str`, not `Int(0)` and not the `g_xxxx` unrecorded-literal
//! fallback token.
//!
//! NOTE: the sibling resolver `resolve_assign_rhs` (used by ASSIGN) has its
//! own `Operand::Global` arm, but it does NOT call `resolve_symbol_name` --
//! it falls back to the raw `g_xxxx` token when no register happens to be
//! bound at that key, which is the normal case for a fresh literal. So
//! `assign x = "hello"; return x;` does NOT currently round-trip "hello"
//! (confirmed empirically while investigating this task). That is a
//! separate, still-open bug in a different function with a different blast
//! radius (also touches MAKE_ARRAY elements and intra-program CALL args) --
//! out of scope here and already tracked separately; see this task's report.
//! `assign_then_return` below is shaped to avoid that path.

use cubelang::compiler;
use cubelang::vm::{ExecResult, VM, Value};

// `assign_then_return` below is deliberately NOT `assign x = "hello"; return
// x;`: that would route the string literal through `resolve_assign_rhs`
// (ASSIGN's RHS resolver), which has its own, separate, still-open bug --
// its `Operand::Global` arm falls back to the raw `g_xxxx` token instead of
// resolving via `resolve_symbol_name`, confirmed empirically while
// investigating this task (see the report's CONCERNS). Fixing that is out
// of scope here (a different function, a different blast radius: MAKE_ARRAY
// elements and intra-program CALL args also go through it) and already
// tracked separately. This scenario instead does an unrelated ASSIGN (int,
// unaffected by either bug) to stay a genuine "a program that assigns, then
// returns a string", while isolating the actual fix under test: RETURN of a
// fresh string literal via `resolve_value`. (CubeLang comments are `#`, not
// `//` -- kept out of the embedded source below entirely to avoid a parse
// error and keep the program minimal.)
const PROGRAM: &str = r#"
program ReturnStringLiteral implements ISolve {
    public function solve(input: str): str {
        return "ok";
    }

    public function return_cat(): str {
        return "cat";
    }

    public function assign_then_return(): str {
        create attempts : number;
        assign attempts = 1;
        return "hello";
    }

    public function return_int(): number {
        return 42;
    }
}
"#;

fn compile_and_load() -> (VM, String) {
    let mut progs = compiler::compile(PROGRAM).expect("compile");
    let prog = progs.pop().expect("exactly one program");
    let name = prog.name.clone();
    let mut vm = VM::new();
    vm.load(prog);
    (vm, name)
}

fn run(func: &str) -> ExecResult {
    let (mut vm, name) = compile_and_load();
    vm.call(&name, func, vec![])
}

#[test]
fn return_of_a_string_literal_yields_the_real_string() {
    match run("solve") {
        ExecResult::Return(Value::Str(s)) | ExecResult::Ok(Value::Str(s)) => {
            assert_eq!(s, "ok", "return \"ok\"; must yield Value::Str(\"ok\"), not Int(0) or a g_xxxx token");
        }
        other => panic!("expected Return/Ok(Str(\"ok\")), got {:?}", other),
    }
}

#[test]
fn return_of_a_different_string_literal_is_not_a_constant_zero() {
    // A second, differently-spelled literal, to rule out a hardcoded/constant
    // answer standing in for real resolution (mirrors asm_vsa.rs's b2 control).
    match run("return_cat") {
        ExecResult::Return(Value::Str(s)) | ExecResult::Ok(Value::Str(s)) => {
            assert_eq!(s, "cat");
        }
        other => panic!("expected Return/Ok(Str(\"cat\")), got {:?}", other),
    }
}

#[test]
fn a_program_that_assigns_then_returns_a_string_literal_works() {
    // "A program", not a bare one-liner: an ASSIGN happens first (int,
    // exercising ordinary register bookkeeping), then RETURN yields a fresh
    // string literal -- the actual `resolve_value` fix under test, in a
    // slightly more realistic shape than `solve`'s single `return "ok";`.
    match run("assign_then_return") {
        ExecResult::Return(Value::Str(s)) | ExecResult::Ok(Value::Str(s)) => {
            assert_eq!(s, "hello");
        }
        other => panic!("expected Return/Ok(Str(\"hello\")), got {:?}", other),
    }
}

#[test]
fn return_of_an_int_literal_is_unaffected_guard() {
    match run("return_int") {
        ExecResult::Return(Value::Int(n)) | ExecResult::Ok(Value::Int(n)) => {
            assert_eq!(n, 42);
        }
        other => panic!("expected Return/Ok(Int(42)), got {:?}", other),
    }
}
