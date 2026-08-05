//! Bugfix B: `Engine::resolve_assign_rhs` -- the resolver shared by ASSIGN,
//! MAKE_ARRAY element resolution, and intra-program CALL argument resolution
//! -- has an `Operand::Global` arm that does NOT call `resolve_symbol_name`.
//! Instead it looks up a register keyed by the operand's raw `g_xxxx` hash
//! token and, when nothing happens to be bound there (the normal case for a
//! fresh string literal -- nothing ever assigns INTO that key), falls back to
//! `Value::Str(<the raw g_xxxx token>)` rather than the literal's real
//! spelling:
//!
//! ```ignore
//! Some(o @ Operand::Global(_)) => {
//!     let key = o.as_name();
//!     self.registers.get(&key).cloned().unwrap_or(Value::Str(key))
//! }
//! ```
//!
//! So `assign x = "hello"; return x;` yields `Value::Str("g_ea8f")` (or
//! whatever "hello" hashes to), not `Value::Str("hello")` -- confirmed
//! empirically while investigating this task. This is the ASSIGN-side twin
//! of the RETURN-side bug `tests/return_string_literal.rs` already pins
//! (`resolve_value`'s missing `Global` arm, fixed separately). Because
//! `resolve_assign_rhs` also backs MAKE_ARRAY's element resolution
//! (`engine.rs`, `op::MAKE_ARRAY`) and the shared `arg_values` intra-program
//! CALL uses (`op::CALL`'s `is_valid_override` and plain-`own_fn` branches),
//! the same bad fallback corrupts string array elements and string-literal
//! CALL arguments too -- a wider blast radius than the RETURN bug.
//!
//! The fix mirrors Bugfix A exactly: resolve a `Global` operand via
//! `resolve_symbol_name` (the shared name-table reversal `resolve_value` and
//! BIND_ROLE/UNBIND's role/filler symbols already use), which falls back to
//! the `g_xxxx` token only for a genuinely unrecorded literal -- not the
//! normal case, since `emit_global` always records the spelling via
//! `record_symbol` before emitting the operand.
//!
//! These tests pin the fix across all three call sites, plus a guard that
//! ASSIGN of a non-literal (an `Imm` int, and a `Named` register-to-register
//! copy) is unaffected -- those arms are untouched by this fix.

use cubelang::compiler;
use cubelang::vm::{ExecResult, VM, Value};

const PROGRAM: &str = r#"
program AssignGlobalLiteral implements ISolve {
    public function solve(input: str): str {
        return "ok";
    }

    public function assign_hello(): str {
        create result : str;
        assign result = "hello";
        return result;
    }

    public function assign_world(): str {
        create result : str;
        assign result = "world";
        return result;
    }

    public function array_first_element(): str {
        let xs = ["alpha", "beta"];
        let y = xs[0];
        return y;
    }

    public function array_second_element(): str {
        let xs = ["alpha", "beta"];
        let y = xs[1];
        return y;
    }

    public function echo(): str {
        return arg0;
    }

    public function call_echo_with_literal(): str {
        let result = echo("hi there");
        return result;
    }

    public function assign_int_guard(): number {
        create n : number;
        assign n = 42;
        return n;
    }

    public function assign_register_copy_guard(): number {
        create a : number;
        assign a = 5;
        create b : number;
        assign b = a;
        return b;
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
fn assign_of_a_string_literal_yields_the_real_string() {
    match run("assign_hello") {
        ExecResult::Return(Value::Str(s)) | ExecResult::Ok(Value::Str(s)) => {
            assert_eq!(s, "hello",
                "assign x = \"hello\"; return x; must yield Value::Str(\"hello\"), \
                 not a g_xxxx token");
        }
        other => panic!("expected Return/Ok(Str(\"hello\")), got {:?}", other),
    }
}

#[test]
fn assign_of_a_different_string_literal_is_not_a_constant() {
    // A second, differently-spelled literal, to rule out a hardcoded/constant
    // answer standing in for real resolution (mirrors asm_vsa.rs's b2 control
    // and return_string_literal.rs's "cat" control).
    match run("assign_world") {
        ExecResult::Return(Value::Str(s)) | ExecResult::Ok(Value::Str(s)) => {
            assert_eq!(s, "world");
        }
        other => panic!("expected Return/Ok(Str(\"world\")), got {:?}", other),
    }
}

#[test]
fn string_literal_array_element_round_trips() {
    // MAKE_ARRAY's element resolution goes through resolve_assign_rhs too --
    // xs[0] must be the real "alpha", not a g_xxxx token.
    match run("array_first_element") {
        ExecResult::Return(Value::Str(s)) | ExecResult::Ok(Value::Str(s)) => {
            assert_eq!(s, "alpha");
        }
        other => panic!("expected Return/Ok(Str(\"alpha\")), got {:?}", other),
    }
}

#[test]
fn string_literal_array_second_element_is_not_the_first() {
    // Rules out an off-by-nothing pass that only happens to resolve index 0.
    match run("array_second_element") {
        ExecResult::Return(Value::Str(s)) | ExecResult::Ok(Value::Str(s)) => {
            assert_eq!(s, "beta");
        }
        other => panic!("expected Return/Ok(Str(\"beta\")), got {:?}", other),
    }
}

#[test]
fn string_literal_call_arg_resolves_to_the_string() {
    // Intra-program CALL args share resolve_assign_rhs's `arg_values`
    // (engine.rs op::CALL, built once before the override/registry/own_fn
    // branch dispatch) -- echo("hi there") must deliver the real string into
    // the callee's arg0, not a g_xxxx token.
    match run("call_echo_with_literal") {
        ExecResult::Return(Value::Str(s)) | ExecResult::Ok(Value::Str(s)) => {
            assert_eq!(s, "hi there");
        }
        other => panic!("expected Return/Ok(Str(\"hi there\")), got {:?}", other),
    }
}

#[test]
fn assign_of_an_int_literal_is_unaffected_guard() {
    // Imm arm, untouched by this fix.
    match run("assign_int_guard") {
        ExecResult::Return(Value::Int(n)) | ExecResult::Ok(Value::Int(n)) => {
            assert_eq!(n, 42);
        }
        other => panic!("expected Return/Ok(Int(42)), got {:?}", other),
    }
}

#[test]
fn assign_register_to_register_copy_is_unaffected_guard() {
    // Named arm (assign b = a, a plain register-value copy), untouched by
    // this fix -- must still be 5, not Null and not a stringified register
    // name.
    match run("assign_register_copy_guard") {
        ExecResult::Return(Value::Int(n)) | ExecResult::Ok(Value::Int(n)) => {
            assert_eq!(n, 5);
        }
        other => panic!("expected Return/Ok(Int(5)), got {:?}", other),
    }
}
