//! Task 6 (CubeLang foundation cycle): the `asm { ... }` raw bytecode-
//! mnemonic escape hatch, and the `vsa` registry module it exists to build.
//!
//! (a) `asm` lexes/parses/compiles a block of `MNEMONIC operand,
//! operand, ...;` lines directly to bytecode -- mapping each mnemonic to its
//! `compiler::op` byte and each operand to a tagged byte pair via the exact
//! same emitters (`emit_named`/`emit_role`/`emit_global`) every other
//! surface construct uses. Its first real job is reaching UNBIND, which
//! (unlike BIND_ROLE, reachable via the `bind` statement) has no surface
//! statement of its own -- the hand-assembled bytecode test at
//! `engine.rs`'s `unbind_opcode_recovers_filler_into_register` is the
//! source-of-truth layout this mirrors, but via real source text.
//!
//! (b) The `vsa` registry module (`vm::registry`) hides that same BIND_ROLE/
//! UNBIND+cleanup machinery behind an ordinary-looking function call --
//! `recover(reg, role)` -- so a GENERATED program never needs an `asm`
//! block itself; `asm` is for building helpers like this one, not for
//! programs that use them. Planting still goes through the existing `bind`
//! statement (already real, already tested) -- there is deliberately no
//! registry-native `bind` counterpart, since `bind` is itself a reserved
//! opcode keyword and unreachable as a call-expression callee; see the task
//! report for the reasoning.
//!
//! Both `reg` and `role` arrive at a registry-native function by their NAME
//! identity (a register key / a role symbol string), not by resolving to a
//! register's current VALUE -- see `resolve_registry_arg`'s doc in
//! `vm::engine` for why a plain CALL's normal Value-resolution
//! (`resolve_assign_rhs`) can't carry a bareword role like `SUBJECT` through
//! (it isn't, and was never meant to be, a register), and why that resolver
//! is scoped ONLY to the registry-call branch -- intra-program/`override`
//! CALL args and ASSIGN are untouched.

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

// ── (a) asm { BIND_ROLE ...; UNBIND ... } -- the UNBIND round trip, from source ──

// The brief's exact snippet (task-6-brief.md Step 1a), wrapped in the
// minimal program/function shell it needs to compile and run -- the inner
// three statements (create/asm/return) are verbatim.
const ASM_UNBIND_ROUNDTRIP: &str = r#"
program AsmUnbindRoundtrip {
    public function run(): str {
        create frame:number;
        asm { BIND_ROLE frame, SUBJECT, "cat"; UNBIND frame, SUBJECT }
        return frame;
    }
}
"#;

#[test]
fn a1_asm_bind_unbind_roundtrip_recovers_cat_via_source() {
    match run(ASM_UNBIND_ROUNDTRIP, "run") {
        ExecResult::Return(Value::Str(s)) | ExecResult::Ok(Value::Str(s)) => {
            assert_eq!(s, "cat", "asm-compiled BIND_ROLE+UNBIND must recover the planted filler");
        }
        other => panic!("expected Return/Ok(Str(\"cat\")), got {:?}", other),
    }
}

#[test]
fn a2_asm_compiles_to_the_same_bytecode_shape_as_the_hand_assembled_unbind_test() {
    // Byte-exact check against the hand-assembled layout at engine.rs's
    // `unbind_opcode_recovers_filler_into_register`: BIND_ROLE (3 operands:
    // NAMED frame, ROLE SUBJECT, GLOBAL "cat") then UNBIND (2 operands:
    // NAMED frame, ROLE SUBJECT). Operand tags per compiler.rs: NAMED=0x01,
    // ROLE=0x04, GLOBAL=0x05.
    use cubelang::compiler::op;

    fn h(s: &str) -> [u8; 2] {
        let d = blake3::hash(s.as_bytes());
        [d.as_bytes()[0], d.as_bytes()[1]]
    }
    let frame = h("frame");
    let subj = h("SUBJECT");
    let cat = h("cat");

    let mut expected: Vec<u8> = Vec::new();
    expected.push(op::BIND_ROLE); expected.push(3);
    expected.push(0x01); expected.push(frame[0]); expected.push(frame[1]);
    expected.push(0x04); expected.push(subj[0]); expected.push(subj[1]);
    expected.push(0x05); expected.push(cat[0]); expected.push(cat[1]);
    expected.push(op::UNBIND); expected.push(2);
    expected.push(0x01); expected.push(frame[0]); expected.push(frame[1]);
    expected.push(0x04); expected.push(subj[0]); expected.push(subj[1]);

    let source = r#"
        program AsmBytecodeShape {
            public function run(): str {
                asm { BIND_ROLE frame, SUBJECT, "cat"; UNBIND frame, SUBJECT }
                return frame;
            }
        }
    "#;
    let programs = compiler::compile(source).expect("compile");
    let bc = &programs[0].functions[0].bytecode;
    assert!(bc.len() >= expected.len(),
        "asm-compiled bytecode is shorter than the expected hand-assembled prefix");
    assert_eq!(&bc[..expected.len()], &expected[..],
        "asm-compiled BIND_ROLE/UNBIND bytes must match the hand-assembled layout exactly");
}

#[test]
fn a3_unknown_mnemonic_is_a_compile_error() {
    let source = r#"
        program AsmBadMnemonic {
            public function run(): str {
                asm { NOT_A_REAL_OP frame, SUBJECT }
                return frame;
            }
        }
    "#;
    let err = compiler::compile(source).expect_err("an unrecognized asm mnemonic must not compile");
    assert!(err.to_string().contains("NOT_A_REAL_OP"), "{}", err);
}

#[test]
fn a4_asm_with_no_operands_and_multiple_instructions_compiles() {
    // A general smoke test that asm isn't hardcoded to exactly the two
    // BIND_ROLE/UNBIND test operands -- SUM takes one operand, RETURN
    // (via asm) can take zero.
    let source = r#"
        program AsmGeneral {
            public function run(): number {
                create x : number;
                assign x = 7;
                asm { SUM x }
                return x;
            }
        }
    "#;
    match run(source, "run") {
        ExecResult::Return(Value::Int(n)) | ExecResult::Ok(Value::Int(n)) => assert_eq!(n, 7),
        other => panic!("expected Return/Ok(Int(7)), got {:?}", other),
    }
}

// ── (b) `use vsa; recover(reg, role)` -- BIND_ROLE/UNBIND+cleanup behind an ordinary call ──

const VSA_RECOVER: &str = r#"
use vsa;

program VsaRecoverTest implements ISolve {
    public function solve(mention: str): str {
        create frame: number;
        bind frame, SUBJECT, "cat";
        bind frame, OBJECT, "mouse";
        return recover(frame, SUBJECT);
    }

    public function recover_wrong_role(): str {
        create frame: number;
        bind frame, SUBJECT, "cat";
        bind frame, OBJECT, "mouse";
        return recover(frame, OBJECT);
    }

    public function recover_unbound_frame(): str {
        create frame: number;
        return recover(frame, SUBJECT);
    }
}
"#;

#[test]
fn b1_recover_recovers_the_planted_filler_through_an_ordinary_call() {
    match run(VSA_RECOVER, "solve") {
        ExecResult::Return(Value::Str(s)) | ExecResult::Ok(Value::Str(s)) => {
            assert_eq!(s, "cat", "recover(frame, SUBJECT) must return the planted filler");
        }
        other => panic!("expected Return/Ok(Str(\"cat\")), got {:?}", other),
    }
}

#[test]
fn b2_a_different_bound_role_is_the_control_and_returns_a_different_symbol() {
    // Proves real recovery, not a hardcoded/constant answer: a DIFFERENT
    // role bound in the SAME frame recovers a DIFFERENT filler.
    match run(VSA_RECOVER, "recover_wrong_role") {
        ExecResult::Return(Value::Str(s)) | ExecResult::Ok(Value::Str(s)) => {
            assert_eq!(s, "mouse");
            assert_ne!(s, "cat", "control must differ from the SUBJECT recovery");
        }
        other => panic!("expected Return/Ok(Str(\"mouse\")), got {:?}", other),
    }
}

#[test]
fn b3_an_unbound_frame_is_the_control_and_recovers_no_symbol() {
    match run(VSA_RECOVER, "recover_unbound_frame") {
        ExecResult::Return(Value::Null) | ExecResult::Ok(Value::Null) => {}
        other => panic!("a frame that was never bound must recover no symbol, got {:?}", other),
    }
}

#[test]
fn b4_compiled_program_carries_vsa_in_used_modules() {
    let progs = compiler::compile(VSA_RECOVER).expect("compile");
    assert_eq!(progs[0].used_modules, vec!["vsa".to_string()]);
}

#[test]
fn b5_calling_recover_without_use_vsa_is_a_runtime_error() {
    // Deny-by-default (Task 3's rule, reused as-is by Task 6): `vsa` must
    // actually be `use`'d, even though the registry always knows it.
    let source = r#"
        program NoUseVsa {
            public function run(): str {
                create frame: number;
                bind frame, SUBJECT, "cat";
                return recover(frame, SUBJECT);
            }
        }
    "#;
    match run(source, "run") {
        ExecResult::Error(e) => assert!(e.contains("recover"), "{}", e),
        other => panic!("deny-by-default: expected an Error, got {:?}", other),
    }
}

#[test]
fn b6_isolve_implements_validates_with_the_registry_interface() {
    // Task 4's ISolve requires exactly one member: `solve` with arity 1.
    // VsaRecoverTest's extra functions (recover_wrong_role,
    // recover_unbound_frame) are unconstrained by the interface.
    compiler::compile(VSA_RECOVER).expect("implements ISolve must validate");
}
