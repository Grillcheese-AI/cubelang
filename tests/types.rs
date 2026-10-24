//! Type checking (2026-10-24). Before, a declared type was decoration: `solve(n: i64)` took "seven"
//! and nothing noticed. Now:
//! - at compile time, a literal is checked where the compiler sees it: a `return`, a typed `let`, a
//!   literal argument to one of the program's own functions, a struct literal's fields;
//! - at run time, every value crossing a function boundary is checked: the host's arguments (text read
//!   as the declared scalar first: "5" for an `i64`), every intra-program call's arguments, every return;
//! - under strict (the default), a type name nothing declares is an error.
//! `null` is a value of every type, and the lenient names (`number`, `symbol`, `quantity`, ...)
//! check nothing, so the programs already in use keep running.

use cubelang::compiler;
use cubelang::vm::{ExecResult, VM, Value};

const TYPED: &str = r#"
struct Point {
    x: i64;
    y: i64 = 0;
}

program Typed implements ISolve {
    type Count = u8;

    public function double(n: i64): i64 {
        let d = n;
        add d, n;
        return d;
    }
    public function norm(p: Point): i64 {
        let s = p.x;
        add s, p.y;
        return s;
    }
    public function says(x: str): i64 { return x; }

    @external
    public function solve(n: i64): i64 {
        let d = double(n);
        return d;
    }
    @external
    public function flag(b: bool): bool { return b; }
    @external
    public function counted(c: Count): i64 { return c; }
    @external
    public function wrong_return(x: str): i64 {
        let r = says(x);
        return r;
    }
    @external
    public function wrong_argument(x: str): i64 {
        let r = double(x);
        return r;
    }
    @external
    public function bad_struct(x: str): i64 {
        let m = { x: 1, z: 2 };
        let r = norm(m);
        return r;
    }
    @external
    public function good_struct(x: str): i64 {
        let m = { x: 4 };
        let r = norm(m);
        return r;
    }
    @external
    public function nothing(x: str): i64 { return null; }
    @external
    public function lenient(x: number): number { return x; }
}
"#;

fn load() -> VM {
    let mut progs = compiler::compile_strict(TYPED).unwrap_or_else(|e| panic!("{e}"));
    let mut vm = VM::new();
    vm.load(progs.remove(0));
    vm
}

fn call(vm: &mut VM, f: &str, arg: Value) -> ExecResult {
    vm.call("Typed", f, vec![arg])
}

fn ok(r: ExecResult) -> Value {
    match r {
        ExecResult::Return(v) | ExecResult::Ok(v) => v,
        other => panic!("{other:?}"),
    }
}

fn err(r: ExecResult) -> String {
    match r {
        ExecResult::Error(e) => e,
        other => panic!("expected a type error, got {other:?}"),
    }
}

#[test]
fn a_hosts_text_is_read_as_the_declared_scalar() {
    let mut vm = load();
    assert!(matches!(ok(call(&mut vm, "solve", Value::Str("21".into()))), Value::Int(42)));
    assert!(matches!(ok(call(&mut vm, "flag", Value::Str("true".into()))), Value::Bool(true)));
    let e = err(call(&mut vm, "solve", Value::Str("seven".into())));
    assert!(e.contains("argument `n` is declared i64") && e.contains("\"seven\""), "{e}");
}

#[test]
fn a_fixed_width_int_carries_its_range_through_an_alias() {
    let mut vm = load();
    assert!(matches!(ok(call(&mut vm, "counted", Value::Str("200".into()))), Value::Int(200)));
    assert!(err(call(&mut vm, "counted", Value::Str("300".into()))).contains("out of range for u8"));
}

#[test]
fn every_return_is_checked() {
    let mut vm = load();
    let e = err(call(&mut vm, "wrong_return", Value::Str("hi".into())));
    assert!(e.contains("says is declared to return i64"), "{e}");
}

#[test]
fn an_intra_program_calls_arguments_are_checked() {
    let mut vm = load();
    let e = err(call(&mut vm, "wrong_argument", Value::Str("hi".into())));
    assert!(e.contains("Typed.double: argument `n` is declared i64"), "{e}");
}

#[test]
fn a_struct_parameter_is_checked_structurally() {
    let mut vm = load();
    assert!(err(call(&mut vm, "bad_struct", Value::Str("q".into()))).contains("Point has no field z"));
    assert!(matches!(ok(call(&mut vm, "good_struct", Value::Str("q".into()))), Value::Int(4)),
            "a map with the struct's fields (a defaulted one left out) is a Point");
}

#[test]
fn null_passes_and_lenient_names_check_nothing() {
    let mut vm = load();
    assert!(matches!(ok(call(&mut vm, "nothing", Value::Str("q".into()))), Value::Null));
    assert!(matches!(ok(call(&mut vm, "lenient", Value::Str("anything".into()))), Value::Str(_)));
}

fn compile_fails(src: &str, says: &str) {
    for e in [compiler::compile_strict(src).err(), compiler::compile(src).err().map(|e| e.to_string())] {
        let e = e.unwrap_or_else(|| panic!("must not compile:\n{src}"));
        assert!(e.contains(says), "{e}");
    }
}

#[test]
fn a_literal_of_the_wrong_type_does_not_compile() {
    compile_fails(r#"
program P implements ISolve {
    @external
    public function solve(x: str): i64 { return "seven"; }
}
"#, "the return value of P::solve is declared i64 but is given str");
    compile_fails(r#"
program P implements ISolve {
    public function half(n: i64): i64 { return n; }
    @external
    public function solve(x: str): i64 {
        let r = half("ten");
        return r;
    }
}
"#, "argument 1 of half() is declared i64 but is given str");
    compile_fails(r#"
program P implements ISolve {
    @external
    public function solve(x: str): i64 {
        let n: bool = 7;
        return 0;
    }
}
"#, "`let n` is declared bool but is given");
}

#[test]
fn an_unknown_type_name_is_a_strict_error() {
    let src = r#"
program P implements ISolve {
    @external
    public function solve(x: Sentense): str { return "ok"; }
}
"#;
    let e = compiler::compile_strict(src).err().expect("strict refuses an undeclared type");
    assert!(e.contains("unknown type `Sentense`"), "{e}");
    assert!(compiler::compile(src).is_ok(), "loose compiles it (and checks nothing for it)");
}
