//! Fix list P6-1 and P6-8 (2026-11-13): a comparison cannot pass by accident, and a long list fails loudly.
//!
//! CubbyLLM's order 0067 probe (`validation/logs/vm_ops_probe_2026-11-13.json` there) ran on `3f37515` and found four
//! programs that returned a wrong value with no error: a missing value (null: an index past the end, an index into a
//! string, a name never written) or the number 0 compared equal to any string. P6-1: such a comparison fails the run
//! with `TypeMismatch`; an index past a list's end fails with `IndexOutOfRange`; a name never declared is a strict
//! compile error (`UndeclaredName`). P6-8: a list literal of 254 or more items used to run as nothing (254 strings)
//! or misread its operands (255 numbers): the operand count is one byte. It is now a compile error, `TooLong`.
use cubelang::compiler;
use cubelang::vm::{ExecResult, Value, VM};

fn source(body: &str) -> String {
    format!("program P implements ISolve {{\n    public function solve(mention: str): number {{\n        {}\n    }}\n}}\n", body)
}

fn run_src(src: &str) -> ExecResult {
    let mut progs = compiler::compile_strict(src).unwrap_or_else(|e| panic!("compiles strict: {}\n{}", e, src));
    let prog = progs.pop().unwrap();
    let name = prog.name.clone();
    let mut vm = VM::new();
    vm.load(prog);
    let _ = vm.call(&name, "constructor", vec![]);
    vm.call(&name, "solve", vec![Value::Str("probe".into())])
}

fn run(body: &str) -> ExecResult {
    run_src(&source(body))
}

fn value(r: ExecResult) -> i64 {
    match r {
        ExecResult::Ok(v) | ExecResult::Return(v) => v.as_i64(),
        other => panic!("expected a value, got {:?}", other),
    }
}

fn error(r: ExecResult) -> String {
    match r {
        ExecResult::Error(e) => e,
        other => panic!("expected an error, got {:?}", other),
    }
}

const N: &str = "create r: number; assign r = 0;";

// ── The probe's four wrong values (3f37515): each now stops with its code ──────────────────────────────

const PREFIX_MISS: &str = r#"create ok: number; assign ok = 1; let s = "hello"; let p = "hx"; let n = p.len();
    create i: number; assign i = 0; while (i < n) { let a = s[i]; let b = p[i]; if (a == b) { } else { assign ok = 0; }
    add i, 1; } return ok;"#;
const SUFFIX_MISS: &str = r#"create ok: number; assign ok = 1; let s = "hello"; let p = "xo";
    let n = p.len(); let m = s.len(); create i: number; assign i = 0; create j: number; while (i < n) { assign j = m;
    sub j, n; add j, i; let a = s[j]; let b = p[i]; if (a == b) { } else { assign ok = 0; } add i, 1; } return ok;"#;
const CHARS_UNEQUAL: &str = r#"create r: number; assign r = 0; let s = "ab"; let a = s[0]; let b = s[1];
    if (a == b) { assign r = 1; } return r;"#;
const CAPITALS: &str = r#"create caps: number; assign caps = 0; let s = "HeLLo";
    let up = ["A","B","C","D","E","F","G","H","I","J","K","L","M","N","O","P","Q","R","S","T","U","V","W","X","Y","Z"];
    let n = s.len(); create i: number; assign i = 0; while (i < n) { let c = s[i]; for (let u of up) {
    if (c == u) { add caps, 1; } } add i, 1; } return caps;"#;

#[test]
fn the_probe_s_four_wrong_values_no_longer_come_out() {
    // 3f37515 gave 1, 1, 1 and 130 (wanted 0, 0, 0 and 3), every one from a null that compared equal to a string.
    for body in [PREFIX_MISS, SUFFIX_MISS, CHARS_UNEQUAL, CAPITALS] {
        let r = run(body);
        if let ExecResult::Error(e) = &r {
            assert!(e.starts_with("TypeMismatch") || e.starts_with("IndexOutOfRange"), "{}", e);
        } else {
            let v = value(r);
            assert!(v == 0 || v == 3, "a wrong value came out again: {} for {}", v, body);
        }
    }
}

// ── P6-1: null, 0 and strings ─────────────────────────────────────────────────────────────────────────────

#[test]
fn zero_against_a_string_fails() {
    assert!(error(run(&format!("{} if (0 == \"x\") {{ assign r = 1; }} return r;", N))).starts_with("TypeMismatch"));
    assert!(error(run(&format!("{} if (0 != \"x\") {{ assign r = 1; }} return r;", N))).starts_with("TypeMismatch"));
}

#[test]
fn null_against_a_string_fails() {
    assert!(error(run(&format!("{} if (null != \"x\") {{ assign r = 1; }} return r;", N))).starts_with("TypeMismatch"));
    assert!(error(run(&format!("{} create c: str; if (c == \"x\") {{ assign r = 1; }} return r;", N)))
        .starts_with("TypeMismatch"));
    // a map's absent key is null: it cannot pass either
    assert!(error(run(&format!("{} let m = {{\"k\": 5}}; let v = m[\"q\"]; if (v < 7) {{ assign r = 1; }} return r;", N)))
        .starts_with("TypeMismatch"));
}

#[test]
fn strings_and_numbers_compare_as_before() {
    assert_eq!(value(run(&format!("{} if (\"x\" == \"x\") {{ assign r = 1; }} return r;", N))), 1);
    assert_eq!(value(run(&format!("{} if (\"x\" == \"y\") {{ assign r = 1; }} return r;", N))), 0);
    assert_eq!(value(run(&format!("{} let a = \"x\"; if (a != \"y\") {{ assign r = 1; }} return r;", N))), 1);
    assert_eq!(value(run(&format!("{} if (mention == \"probe\") {{ assign r = 1; }} return r;", N))), 1);
    for (cond, want) in [("2 < 3", 1), ("3 <= 3", 1), ("3 > 2", 1), ("2 >= 3", 0), ("3 == 3", 1), ("3 != 3", 0)] {
        assert_eq!(value(run(&format!("{} if ({}) {{ assign r = 1; }} return r;", N, cond))), want, "{}", cond);
    }
    let vars = "create a: number; assign a = 4; create b: number; assign b = 3;";
    assert_eq!(value(run(&format!("{} {} if (a < b) {{ assign r = 1; }} return r;", N, vars))), 0);
    assert_eq!(value(run(&format!("{} {} if (a > b) {{ assign r = 1; }} return r;", N, vars))), 1);
    // truthiness is not a comparison and keeps its meaning; `if (t)` used to run as `if (t == 6)` (the kind was
    // emitted where COMPARE reads its second operand)
    for (t, want) in [(2, 1), (0, 0), (6, 1), (7, 1)] {
        let body = format!("{} create t: number; assign t = {}; if (t) {{ assign r = 1; }} return r;", N, t);
        assert_eq!(value(run(&body)), want, "if ({})", t);
    }
}

#[test]
fn an_index_past_a_list_s_end_fails() {
    let e = error(run("let xs = [4, 9, 2]; let v = xs[3]; return v;"));
    assert!(e.starts_with("IndexOutOfRange"), "{}", e);
    let e = error(run("let xs = [4, 9, 2]; create i: number; assign i = 0; sub i, 1; let v = xs[i]; return v;"));
    assert!(e.starts_with("IndexOutOfRange"), "{}", e);
    assert_eq!(value(run("let xs = [4, 9, 2]; create i: number; assign i = 2; let v = xs[i]; return v;")), 2);
}

#[test]
fn a_name_never_declared_is_a_strict_compile_error() {
    let src = source(&format!("{} if (mentoin == \"probe\") {{ assign r = 1; }} return r;", N));
    let e = compiler::compile_strict(&src).err().expect("an undeclared name must not compile under strict");
    assert!(e.contains("UndeclaredName") && e.contains("mentoin"), "{}", e);
    // loose, it compiles, and the read is a missing value: the comparison fails rather than passing
    let mut progs = compiler::compile(&src).expect("compiles loose");
    let prog = progs.pop().unwrap();
    let mut vm = VM::new();
    vm.load(prog);
    assert!(error(vm.call("P", "solve", vec![Value::Str("probe".into())])).starts_with("TypeMismatch"));
}

#[test]
fn declared_names_still_compile_in_every_position() {
    // a parameter, a let, a create, a for binding, a recall's register, a positional argument, a role symbol
    let src = r#"
use vsa;
program P implements ISolve {
    function twice(x: number): number { create y: number; assign y = arg0; mul y, 2; return y; }
    public function solve(mention: str): number {
        create r: number; assign r = 0;
        let xs = [1, 2];
        for (let x of xs) { add r, x; }
        create frame: number;
        bind frame, SUBJECT, "cat";
        let who = recover(frame, SUBJECT);
        if (who == "cat") { add r, 10; }
        if (mention == "probe") { add r, 100; }
        return r;
    }
}
"#;
    assert_eq!(value(run_src(src)), 113);
}

#[test]
fn qc_decision_still_validates_and_decides_on_injected_inputs() {
    let src = include_str!("../examples/qc_decision.cube");
    let report = cubelang::validate::validate(src).expect("parses");
    assert!(report.programs.iter().all(|p| p.is_clean()), "{}", report);
    let mut progs = compiler::compile_strict(src).expect("compiles strict");
    let prog = progs.pop().unwrap();
    for (class, conf, want) in [(0, 95, 0), (3, 95, 1), (0, 40, 2)] {
        let mut vm = VM::new();
        vm.load(prog.clone());
        let _ = vm.init("QCDecision");
        vm.registers.insert(VM::register_key("in_class"), Value::Int(class));
        vm.registers.insert(VM::register_key("in_conf"), Value::Int(conf));
        assert_eq!(value(vm.call("QCDecision", "solve", vec![])), want, "class {} conf {}", class, conf);
    }
}

// ── P6-8: a long list fails loudly ───────────────────────────────────────────────────────────────────────

fn list_of(n: usize, strings: bool) -> String {
    let items: Vec<String> = (0..n).map(|i| if strings { format!("\"w{}\"", i) } else { (i % 7).to_string() }).collect();
    source(&format!("let xs = [{}]; let n = xs.len(); return n;", items.join(", ")))
}

#[test]
fn a_list_of_253_items_runs_and_254_or_more_is_too_long() {
    // the edge: an instruction carries at most 255 operands, the list's register and length take two
    for strings in [false, true] {
        assert_eq!(value(run_src(&list_of(253, strings))), 253, "253 items, strings {}", strings);
        for n in [254, 255, 256, 300] {
            let e = compiler::compile_strict(&list_of(n, strings)).err()
                .unwrap_or_else(|| panic!("{} items (strings {}) must not compile", n, strings));
            assert!(e.contains("TooLong") && e.contains("at most 253"), "{}", e);
            let e = compiler::compile(&list_of(n, strings)).err().expect("loose compile refuses it too");
            assert!(e.message.contains("TooLong"), "{}", e.message);
        }
    }
}

#[test]
fn a_call_with_too_many_arguments_is_too_long() {
    let args: Vec<String> = (0..255).map(|i| i.to_string()).collect();
    let src = format!("program P implements ISolve {{\n function f(a: number): number {{ return a; }}\n \
        public function solve(mention: str): number {{ let v = f({}); return v; }}\n}}\n", args.join(", "));
    let e = compiler::compile(&src).err().expect("255 arguments do not fit one CALL");
    assert!(e.message.contains("TooLong"), "{}", e.message);
}
