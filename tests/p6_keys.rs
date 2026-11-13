//! Fix list P6-9 (2026-11-13): two different strings never share a key.
//!
//! Every identifier and string literal used to compile to BLAKE3's first two bytes, so `"thing" == "government"`
//! was true (both `15d0`; also `"eptwcv"` / `"rmcens"`), and two variables with one key shared a register. Keys are
//! eight bytes now (`vm::key`); a program from a `.cubebin` written before version 5 keeps its two-byte keys and
//! still runs.
use cubelang::compiler::{self, CompiledProgram};
use cubelang::vm::key::Key;
use cubelang::vm::{ExecResult, Value, VM};

fn run_src(src: &str, func: &str) -> ExecResult {
    let mut progs = compiler::compile_strict(src).unwrap_or_else(|e| panic!("compiles strict: {}\n{}", e, src));
    let prog = progs.pop().unwrap();
    let name = prog.name.clone();
    let mut vm = VM::new();
    vm.load(prog);
    let _ = vm.call(&name, "constructor", vec![]);
    vm.call(&name, func, vec![Value::Str("probe".into())])
}

fn body(b: &str) -> String {
    format!("program P implements ISolve {{\n    public function solve(mention: str): number {{\n        {}\n    }}\n}}\n", b)
}

fn value(r: ExecResult) -> i64 {
    match r {
        ExecResult::Ok(v) | ExecResult::Return(v) => v.as_i64(),
        other => panic!("expected a value, got {:?}", other),
    }
}

#[test]
fn the_pairs_found_shared_a_two_byte_key() {
    assert_eq!(Key::narrow("thing"), Key::narrow("government"));
    assert_eq!(Key::narrow("eptwcv"), Key::narrow("rmcens"));
}

#[test]
fn the_smallest_program_returns_0() {
    // 0067's smallest case: on the two-byte keys this returned 1
    let src = body(r#"create r: number; assign r = 0; if ("thing" == "government") { assign r = 1; } return r;"#);
    assert_eq!(value(run_src(&src, "solve")), 0);
}

#[test]
fn the_two_pairs_found_compare_unequal_and_each_equals_itself() {
    for (a, b) in [("thing", "government"), ("eptwcv", "rmcens")] {
        let ne = body(&format!(r#"create r: number; assign r = 0; let x = "{}"; if (x == "{}") {{ assign r = 1; }} return r;"#, a, b));
        assert_eq!(value(run_src(&ne, "solve")), 0, "{} == {}", a, b);
        let eq = body(&format!(r#"create r: number; assign r = 0; let x = "{}"; if (x == "{}") {{ assign r = 1; }} return r;"#, a, a));
        assert_eq!(value(run_src(&eq, "solve")), 1, "{} == {}", a, a);
    }
}

#[test]
fn two_colliding_variable_names_keep_their_own_values() {
    let src = body("create thing: number; assign thing = 1; create government: number; assign government = 2; \
                    create r: number; assign r = 0; add r, thing; mul r, 10; add r, government; return r;");
    assert_eq!(value(run_src(&src, "solve")), 12);
    let src = body("create eptwcv: number; assign eptwcv = 3; create rmcens: number; assign rmcens = 4; return eptwcv;");
    assert_eq!(value(run_src(&src, "solve")), 3);
}

#[test]
fn a_cubebin_round_trip_keeps_the_eight_byte_keys() {
    let src = body(r#"create r: number; assign r = 0; let x = "thing"; if (x == "government") { assign r = 1; } return r;"#);
    let prog = compiler::compile_strict(&src).unwrap().pop().unwrap();
    assert!(!prog.narrow_keys);
    let bin = prog.to_cubebin();
    assert_eq!(bin[4], compiler::CUBEBIN_VERSION);
    assert_eq!(compiler::CUBEBIN_VERSION, 5);
    let back = CompiledProgram::from_cubebin(&bin).expect("reads back");
    assert!(!back.narrow_keys);
    let mut vm = VM::new();
    vm.load(back);
    assert_eq!(value(vm.call("P", "solve", vec![Value::Str("probe".into())])), 0);
}

#[test]
fn a_pre_v5_cubebin_still_runs_on_its_two_byte_keys() {
    // examples/qc_decision.cubebin is a version-1 snapshot: its bytecode keys names by two bytes
    let bin = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/qc_decision.cubebin")).unwrap();
    assert!(bin[4] < 5);
    let prog = CompiledProgram::from_cubebin(&bin).expect("a v1 file loads");
    assert!(prog.narrow_keys);
    for (class, conf, want) in [(0, 95, 0), (3, 95, 1), (0, 40, 2)] {
        let mut vm = VM::new();
        vm.load(prog.clone());
        let _ = vm.init("QCDecision");
        // a host reaches the program's registers through `register_key_in`, which knows the program's width
        vm.registers.insert(vm.register_key_in("QCDecision", "in_class"), Value::Int(class));
        vm.registers.insert(vm.register_key_in("QCDecision", "in_conf"), Value::Int(conf));
        assert_eq!(value(vm.call("QCDecision", "solve", vec![])), want, "class {} conf {}", class, conf);
    }
}

#[test]
fn a_one_arg_recall_finds_what_store_filed_under_the_same_spelling() {
    // `store x, "estimate"` and `recall estimate` used to address the memory by two different display tokens
    // (`g_<key>` and `r_<key>`) and found each other only when the nearest-key search happened to land there
    let src = body(r#"create a: number; assign a = 40; store a, "estimate"; create other: number; assign other = 7;
        remember other; create b: number; assign b = 65; store b, "estimate"; recall estimate;
        create r: number; assign r = 0; add r, estimate; return r;"#);
    assert_eq!(value(run_src(&src, "solve")), 65);
}
