//! Fix list P6-3 to P6-7 (2026-11-13): the forms a generated program needs.
//!
//! CubbyLLM's order 0067 probe (`validation/logs/vm_ops_probe_2026-11-13.json` there) ran 71 small programs on
//! `3f37515`; 29 failed to compile or run. Each probe program named here is one of them, with the value it should give.
//! - P6-3: `&&`, `||`, `!` and `else if` in conditions, short-circuit.
//! - P6-4: expressions as operands: calls, method calls, indexes and arithmetic inside a condition, a `return`, an
//!   argument, a `let`, an opcode statement.
//! - P6-5: whole-number division `//` and `%`; `div` keeps its meaning.
//! - P6-6: `break` in `for` and `while`.
//! - P6-7: `starts_with`, `ends_with`, `upper`, `lower`, `contains`, and `+` joining strings, in English and French.
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

fn got(body: &str) -> Value {
    match run_src(&source(body)) {
        ExecResult::Ok(v) | ExecResult::Return(v) => v,
        other => panic!("expected a value, got {:?}\n{}", other, body),
    }
}

fn int(body: &str) -> i64 {
    match got(body) {
        Value::Int(n) => n,
        other => panic!("expected a whole number, got {:?}\n{}", other, body),
    }
}

fn error(body: &str) -> String {
    match run_src(&source(body)) {
        ExecResult::Error(e) => e,
        other => panic!("expected an error, got {:?}\n{}", other, body),
    }
}

fn compile_error(body: &str) -> String {
    match compiler::compile_strict(&source(body)) {
        Err(e) => e,
        Ok(_) => panic!("expected a compile error\n{}", body),
    }
}

const R: &str = "create r: number; assign r = 0; create a: number;";

// ── P6-3: &&, ||, !, else if ────────────────────────────────────────────────────────────────────────────────

#[test]
fn and_or_not_both_sides() {
    // probes 13-17
    let and = |a: i64| int(&format!("{} assign a = {}; if (a > 1 && a < 5) {{ assign r = 1; }} return r;", R, a));
    assert_eq!((and(2), and(7), and(0)), (1, 0, 0));
    let or = |a: i64| int(&format!("{} assign a = {}; if (a < 1 || a > 5) {{ assign r = 1; }} return r;", R, a));
    assert_eq!((or(7), or(0), or(3)), (1, 1, 0));
    let not = |a: i64| int(&format!("{} assign a = {}; if (!(a > 5)) {{ assign r = 1; }} return r;", R, a));
    assert_eq!((not(3), not(7)), (1, 0));
}

#[test]
fn else_if_three_ways() {
    // probe 18 (a = 3 wants 2)
    let pick = |a: i64| int(&format!(
        "{} assign a = {}; if (a > 5) {{ assign r = 1; }} else if (a > 2) {{ assign r = 2; }} else {{ assign r = 3; }} return r;",
        R, a));
    assert_eq!((pick(7), pick(3), pick(1)), (1, 2, 3));
}

#[test]
fn and_or_short_circuit() {
    // the right side would fail (IndexOutOfRange); it never runs when the left side decides
    let s = r#"let s = "ab"; create i: number; assign i = 5;"#;
    assert_eq!(int(&format!(r#"{} {} if (i < 2 && s[i] == "a") {{ assign r = 1; }} return r;"#, R, s)), 0);
    assert_eq!(int(&format!(r#"{} {} if (i > 2 || s[i] == "a") {{ assign r = 1; }} return r;"#, R, s)), 1);
    // and the right side does run when it decides
    assert!(error(&format!(r#"{} {} if (i > 2 && s[i] == "a") {{ assign r = 1; }} return r;"#, R, s)).contains("IndexOutOfRange"));
}

#[test]
fn a_condition_as_a_value_is_one_or_zero() {
    assert_eq!(int(&format!("{} assign a = 3; let b = a > 1 && !(a == 4); return b;", R)), 1);
    assert_eq!(int(&format!("{} assign a = 3; let b = a > 5 || a == 4; return b;", R)), 0);
}

// ── P6-4: expressions as operands ───────────────────────────────────────────────────────────────────────────

#[test]
fn the_probes_operand_spellings() {
    assert_eq!(int("let xs = [1, 2, 3, 4]; return xs.len();"), 4); // probe 2
    assert_eq!(int("let xs = [4, 9, 2]; create i: number; assign i = 1; return xs[i];"), 9); // probe 3
    assert_eq!(int("create a: number; assign a = 3; create b: number; assign b = 4; let c = a + b * 2; return c;"), 11); // 21
    assert_eq!(int("create r: number; assign r = 0; let xs = [[1, 2], [3]]; for (let row of xs) { add r, row.len(); } return r;"), 3); // 24
    assert_eq!(int(r#"let s = "hello"; return s.len();"#), 5); // probe 26
    assert_eq!(int(r#"create r: number; assign r = 0; let s = "hello"; if (s[0] == "h") { assign r = 1; } return r;"#), 1); // 28
    assert_eq!(int(r#"let m = {"k": 5}; return m["k"];"#), 5); // probe 41
    // probe 42, which worked before: the host's string still compares (the harness passes "probe")
    assert_eq!(int(r#"create r: number; assign r = 0; if (mention == "probe") { assign r = 1; } return r;"#), 1);
}

#[test]
fn a_function_with_parameters() {
    // probe 25
    let src = "program P implements ISolve {\n    function twice(x: number): number { let y = x * 2; return y; }\n    \
               public function solve(mention: str): number { create r: number; assign r = 0; let v = twice(3); return v; }\n}\n";
    match run_src(src) {
        ExecResult::Ok(v) | ExecResult::Return(v) => assert_eq!(v.as_i64(), 6),
        other => panic!("{:?}", other),
    }
}

#[test]
fn in_a_condition_a_return_and_an_argument() {
    assert_eq!(int(&format!("{} let xs = [1, 2, 3, 4]; if (xs.len() > 3) {{ assign r = 1; }} return r;", R)), 1);
    assert_eq!(int("create a: number; assign a = 1; create b: number; assign b = 5; return a + b * 2;"), 11);
    assert_eq!(int("let xs = [2, 3]; return xs[0] * xs[1] - (xs[1] - xs[0]);"), 5);
    assert_eq!(int(r#"let s = "hello"; return contains(s, s[1] + s[2]);"#), 1);
    assert_eq!(int("create a: number; assign a = 3; return -a + 10;"), 7);
}

#[test]
fn a_while_condition_is_recomputed_each_turn() {
    assert_eq!(int("create i: number; assign i = 0; while (i * 2 < 10) { add i, 1; } return i;"), 5);
}

#[test]
fn a_nested_list_read_and_a_match_on_a_computed_subject() {
    assert_eq!(int("let xs = [[1, 2], [3, 4, 5]]; let row = xs[1]; return row[2] + xs[0].len();"), 7);
    assert_eq!(int("create r: number; let xs = [1, 2, 3]; match (xs.len() * 2) { 6 => { assign r = 1; } _ => { assign r = 2; } } return r;"), 1);
}

// ── P6-5: // and % ──────────────────────────────────────────────────────────────────────────────────────────

#[test]
fn whole_number_division_and_remainder() {
    assert_eq!(int("create a: number; assign a = 7; let c = a // 2; return c;"), 3);
    assert_eq!(int("create a: number; assign a = 7; let c = a % 2; return c;"), 1); // probe 22
    assert_eq!(int("create a: number; assign a = 0; sub a, 7; let c = a // 2; return c;"), -4);
    assert_eq!(int("create a: number; assign a = 0; sub a, 7; let c = a % 2; return c;"), 1);
    assert_eq!(int("return 6 // 3 + 6 % 4;"), 4);
    assert!(error("create a: number; assign a = 7; let c = a % 0; return c;").contains("division by zero"));
    assert!(error(r#"let s = "x"; let c = s // 2; return c;"#).contains("TypeMismatch"));
}

#[test]
fn div_keeps_its_meaning() {
    // probe 50: `div` is true division, as before
    assert!(matches!(got("create c: number; assign c = 7; div c, 2; return c;"), Value::Float(f) if f == 3.5));
    assert!(matches!(got("create a: number; assign a = 7; let c = a / 2; return c;"), Value::Float(f) if f == 3.5));
}

// ── P6-6: break ─────────────────────────────────────────────────────────────────────────────────────────────

#[test]
fn break_stops_a_loop_at_its_fourth_turn() {
    // probe 23 (while), and the same in a for
    assert_eq!(int("create r: number; assign r = 0; create i: number; assign i = 0; while (i < 10) { add i, 1; if (i == 4) { break; } } return i;"), 4);
    assert_eq!(int("create n: number; assign n = 0; let xs = [1, 2, 3, 4, 5, 6]; for (let x of xs) { add n, 1; if (x == 4) { break; } } return n;"), 4);
}

#[test]
fn break_leaves_only_the_innermost_loop() {
    let body = "create n: number; assign n = 0; let xs = [1, 2, 3]; for (let x of xs) { for (let y of xs) { \
                if (y == 2) { break; } add n, 1; } } return n;";
    assert_eq!(int(body), 3);
}

#[test]
fn break_outside_a_loop_is_a_compile_error() {
    assert!(compile_error("create r: number; assign r = 0; break; return r;").contains("break"));
}

// ── P6-7: string helpers and + ──────────────────────────────────────────────────────────────────────────────

fn hit(cond: &str) -> i64 {
    int(&format!("create r: number; assign r = 0; if ({}) {{ assign r = 1; }} return r;", cond))
}

#[test]
fn string_helpers_hit_and_miss_in_english_and_french() {
    let cases: &[(&str, i64)] = &[
        // the probe's spellings (29, 31, 32, 34, 37, 39)
        (r#""hello".starts_with("he")"#, 1), (r#"starts_with("hello", "he")"#, 1), (r#""hello".starts_with("hx")"#, 0),
        (r#""hello".ends_with("lo")"#, 1), (r#"ends_with("hello", "lo")"#, 1), (r#""hello".ends_with("xo")"#, 0),
        (r#"upper("hello") == "HELLO""#, 1), (r#""HeLLo".lower() == "hello""#, 1), (r#""hello".upper() == "Hello""#, 0),
        (r#""hello".contains("ell")"#, 1), (r#""hello".contains("elo")"#, 0),
        // French, accents kept
        (r#""éléphant".starts_with("élé")"#, 1), (r#""éléphant".starts_with("ele")"#, 0),
        (r#""café".ends_with("fé")"#, 1), (r#""café".ends_with("fe")"#, 0),
        (r#""été".upper() == "ÉTÉ""#, 1), (r#""ÉCOLE".lower() == "école""#, 1), (r#""été".upper() == "ETE""#, 0),
        (r#""garçon".contains("ç")"#, 1), (r#""garçon".contains("c")"#, 0),
    ];
    for (cond, want) in cases {
        assert_eq!(hit(cond), *want, "{}", cond);
    }
}

#[test]
fn plus_joins_two_strings() {
    // probe 40
    assert_eq!(int(r#"create r: number; assign r = 0; let s = "he" + "llo"; if (s == "hello") { assign r = 1; } return r;"#), 1);
    assert_eq!(int(r#"create r: number; assign r = 0; let s = "Ça " + "va"; if (s == "Ça va" && s.len() == 5) { assign r = 1; } return r;"#), 1);
    assert_eq!(hit(r#"("a" + "b") == "ba""#), 0);
}

#[test]
fn a_string_and_a_number_do_not_mix() {
    assert!(error(r#"let s = "a" + 1; return s;"#).contains("TypeMismatch"));
    assert!(error(r#"create r: number; assign r = 0; if ("abc".starts_with(1)) { assign r = 1; } return r;"#).contains("TypeMismatch"));
    assert!(error("create r: number; assign r = 0; if (upper(5) == \"5\") { assign r = 1; } return r;").contains("TypeMismatch"));
}

#[test]
fn a_programs_own_function_wins_over_a_helper() {
    let src = "program P implements ISolve {\n    function upper(x: str): number { return 7; }\n    \
               public function solve(mention: str): number { let v = upper(\"a\"); return v; }\n}\n";
    match run_src(src) {
        ExecResult::Ok(v) | ExecResult::Return(v) => assert_eq!(v.as_i64(), 7),
        other => panic!("{:?}", other),
    }
}
