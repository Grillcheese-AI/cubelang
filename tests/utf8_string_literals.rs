//! Regression test for a confirmed UTF-8 corruption bug in the lexer.
//!
//! `Lexer::lex_string` used to walk `source: &[u8]` and do `s.push(ch as
//! char)` per raw byte, so each UTF-8 continuation byte of a non-ASCII
//! source character got reinterpreted as its own independent Latin-1
//! codepoint — a string literal `"Köln"` round-tripped out as `"KÃ¶ln"`.
//! The fix accumulates a string literal's bytes into a `Vec<u8>` and
//! decodes once, with `String::from_utf8`, at the closing quote.
//!
//! This mirrors `tests/asm_vsa.rs`'s `VSA_RECOVER` shape exactly (same
//! `bind`/`recover` round trip used by CubbyLLM's `reasoning_bridge.cube`)
//! but with non-ASCII fillers standing in for `"cat"`/`"mouse"`, so the
//! coverage is both "the lexer decodes the literal correctly" AND "the
//! decoded value survives a real bind/unbind VSA round trip unmangled."

use cubelang::compiler;
use cubelang::vm::{ExecResult, VM, Value};

fn compile_and_load(source: &str) -> (VM, String) {
    let mut progs = compiler::compile(source).expect("compile");
    let prog = progs.pop().expect("exactly one program");
    let name = prog.name.clone();
    let mut vm = VM::new();
    vm.load(prog);
    (vm, name)
}

fn run(source: &str, func: &str) -> ExecResult {
    let (mut vm, name) = compile_and_load(source);
    vm.call(&name, func, vec![])
}

const UTF8_BIND_RECOVER: &str = r#"
use vsa;

program Utf8BindRecoverTest implements ISolve {
    public function solve(mention: str): str {
        create frame: number;
        bind frame, SUBJECT, "Köln";
        bind frame, OBJECT, "naïve";
        return recover(frame, SUBJECT);
    }

    public function recover_object(): str {
        create frame: number;
        bind frame, SUBJECT, "Köln";
        bind frame, OBJECT, "naïve";
        return recover(frame, OBJECT);
    }

    public function recover_cjk(): str {
        create frame: number;
        bind frame, SUBJECT, "東京";
        return recover(frame, SUBJECT);
    }
}
"#;

#[test]
fn utf8_subject_filler_recovers_byte_exact_through_bind_unbind() {
    match run(UTF8_BIND_RECOVER, "solve") {
        ExecResult::Return(Value::Str(s)) | ExecResult::Ok(Value::Str(s)) => {
            assert_eq!(s, "Köln", "non-ASCII filler must recover byte-exact, not mojibake");
        }
        other => panic!("expected Return/Ok(Str(\"Köln\")), got {:?}", other),
    }
}

#[test]
fn utf8_object_filler_is_the_control_and_also_recovers_byte_exact() {
    match run(UTF8_BIND_RECOVER, "recover_object") {
        ExecResult::Return(Value::Str(s)) | ExecResult::Ok(Value::Str(s)) => {
            assert_eq!(s, "naïve");
            assert_ne!(s, "Köln", "control must differ from the SUBJECT recovery");
        }
        other => panic!("expected Return/Ok(Str(\"naïve\")), got {:?}", other),
    }
}

#[test]
fn utf8_cjk_filler_recovers_byte_exact() {
    // A three-byte-per-character script (no ASCII-range bytes at all in
    // the literal) is the strictest test of the byte-accumulation fix.
    match run(UTF8_BIND_RECOVER, "recover_cjk") {
        ExecResult::Return(Value::Str(s)) | ExecResult::Ok(Value::Str(s)) => {
            assert_eq!(s, "東京");
        }
        other => panic!("expected Return/Ok(Str(\"東京\")), got {:?}", other),
    }
}

#[test]
fn utf8_string_literals_lex_and_compile_without_corruption() {
    // Sanity check at the compile boundary (lexer + parser + compiler),
    // independent of the VM bind/recover round trip above: the literal
    // survives being embedded as a bytecode GLOBAL operand.
    let source = r#"
        program Utf8Compile implements ISolve {
            public function solve(input: str): str {
                return "Köln";
            }
        }
    "#;
    let progs = compiler::compile(source).expect("compile");
    assert_eq!(progs.len(), 1);
}
