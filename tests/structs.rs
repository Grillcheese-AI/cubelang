//! Structs as runtime values (2026-10-24).
//!
//! Before: `struct` declarations parsed and compiled to nothing; `p.x` compiled to a register
//! literally named "p.x" that nothing could write, so every field read silently gave 0; a field
//! assignment and any struct or map literal outside a few `let` shapes were compile errors; `null`
//! compiled to the integer 0; and `create m : map` never actually made an empty map.
//!
//! Now a struct value is a map holding every declared field (defaults filled in), `p.x` reads it,
//! `p.x = v` writes it, and struct, map and array literals work wherever an operand goes.

use cubelang::compiler::{self, CompiledProgram};
use cubelang::vm::{ExecResult, VM, Value};

const SHAPES: &str = r#"
struct Point {
    x: number;
    y: number = 0;
}

struct Labeled {
    name: str;
    at: Point;
    tag: str = "none";
}

program Shapes implements ISolve {
    storage { origin: mutable Point; }

    public function make(px: number, py: number): Point {
        return Point { x: px, y: py };
    }
    public function norm1(p: Point): number {
        let ax = p.x;
        add ax, p.y;
        return ax;
    }

    @external
    public function solve(input: str): number {
        let p = Point { x: 3 };
        p.y = 4;
        let total = p.x;
        add total, p.y;
        return total;
    }
    @external
    public function defaults(input: str): number {
        let p = Point { x: 9 };
        return p.y;
    }
    @external
    public function nested(input: str): str {
        let l = Labeled { name: "home", at: Point { x: 1, y: 2 } };
        return l.at.y;
    }
    @external
    public function tag(input: str): str {
        let l = Labeled { name: "home", at: Point { x: 1 } };
        return l.tag;
    }
    @external
    public function through_calls(input: str): number {
        let q = make(5, 6);
        let n = norm1(q);
        return n;
    }
    @external
    public function literal_argument(input: str): number {
        let n = norm1(Point { x: 7, y: 8 });
        return n;
    }
    @external
    public function maps(input: str): str {
        let m = { name: "cubby", kind: "bear" };
        return m.kind;
    }
    @external
    public function missing_field(input: str): str {
        let m = { name: "cubby" };
        return m.colour;
    }
    @external
    public function storage_field(input: str): number {
        self.origin = Point { x: 11, y: 12 };
        return self.origin.y;
    }
    @external
    public function nothing(input: str): str {
        return null;
    }
    @external
    public function empty_map(input: str): number {
        create m : map<str, number>;
        return m;
    }
    @external
    public function map_len(input: str): number {
        let p = Point { x: 1 };
        let n = p.len();
        return n;
    }
    @external
    public function whole(input: str): Point {
        return Point { x: 1, y: 2 };
    }
}
"#;

fn load() -> VM {
    let mut progs = compiler::compile_strict(SHAPES).unwrap_or_else(|e| panic!("{e}"));
    let mut vm = VM::new();
    vm.load(progs.remove(0));
    vm
}

fn run(vm: &mut VM, f: &str) -> Value {
    match vm.call("Shapes", f, vec![Value::Str("q".into())]) {
        ExecResult::Return(v) | ExecResult::Ok(v) => v,
        other => panic!("{f}: {other:?}"),
    }
}

fn int(v: Value) -> i64 {
    match v {
        Value::Int(n) => n,
        other => panic!("expected an int, got {other:?}"),
    }
}

fn text(v: Value) -> String {
    match v {
        Value::Str(s) => s,
        other => panic!("expected a string, got {other:?}"),
    }
}

#[test]
fn a_field_reads_and_writes() {
    let mut vm = load();
    assert_eq!(int(run(&mut vm, "solve")), 7, "p.x + p.y after p.y = 4");
}

#[test]
fn a_left_out_field_takes_its_default() {
    let mut vm = load();
    assert_eq!(int(run(&mut vm, "defaults")), 0);
    assert_eq!(text(run(&mut vm, "tag")), "none");
}

#[test]
fn structs_nest_and_paths_read_through() {
    let mut vm = load();
    assert_eq!(int(run(&mut vm, "nested")), 2, "l.at.y");
}

#[test]
fn a_struct_passes_into_and_out_of_a_call() {
    let mut vm = load();
    assert_eq!(int(run(&mut vm, "through_calls")), 11);
    assert_eq!(int(run(&mut vm, "literal_argument")), 15, "a struct literal as a call argument");
}

#[test]
fn a_struct_value_is_a_map_of_every_declared_field() {
    let mut vm = load();
    match run(&mut vm, "whole") {
        Value::Map(m) => {
            assert_eq!(m.len(), 2);
            assert!(matches!(m.get("x"), Some(Value::Int(1))));
            assert!(matches!(m.get("y"), Some(Value::Int(2))));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn map_literals_and_a_missing_key() {
    let mut vm = load();
    assert_eq!(text(run(&mut vm, "maps")), "bear");
    assert!(matches!(run(&mut vm, "missing_field"), Value::Null), "a missing key reads as null, not 0");
}

#[test]
fn a_storage_field_can_hold_a_struct() {
    let mut vm = load();
    assert_eq!(int(run(&mut vm, "storage_field")), 12);
}

#[test]
fn null_is_null_and_an_empty_map_is_a_map() {
    let mut vm = load();
    assert!(matches!(run(&mut vm, "nothing"), Value::Null), "`return null;` used to return 0");
    assert!(matches!(run(&mut vm, "empty_map"), Value::Map(m) if m.is_empty()),
            "`create m : map` makes an empty map, not null");
    assert_eq!(int(run(&mut vm, "map_len")), 2, "a struct's len() counts its fields (it used to read 0)");
}

#[test]
fn field_reads_survive_cubebin() {
    let progs = compiler::compile_strict(SHAPES).unwrap();
    let path = std::env::temp_dir().join(format!("structs_round_trip_{}.cubebin", std::process::id()));
    progs[0].save(&path).unwrap();
    let back = CompiledProgram::load(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    let mut vm = VM::new();
    vm.load(back);
    assert_eq!(int(run(&mut vm, "nested")), 2);
}

fn compile_error(body: &str) -> String {
    let src = format!(
        "struct Point {{ x: i64; y: i64 = 0; }}\nprogram P implements ISolve {{\n    @external\n    public function solve(input: str): number {{\n        {body}\n        return 0;\n    }}\n}}\n");
    let strict = compiler::compile_strict(&src).err().unwrap_or_else(|| panic!("must not compile: {body}"));
    let loose = compiler::compile(&src).err().map(|e| e.to_string()).unwrap_or_else(|| panic!("must not compile loose: {body}"));
    assert_eq!(strict.contains("Point") || strict.contains("field") || strict.contains("struct"), true, "{strict}");
    let _ = loose;
    strict
}

#[test]
fn a_malformed_struct_literal_does_not_compile() {
    assert!(compile_error("let p = Point { y: 1 };").contains("leaves out `x`"));
    assert!(compile_error("let p = Point { x: 1, z: 2 };").contains("has no field `z`"));
    assert!(compile_error("let p = Point { x: 1, x: 2 };").contains("twice"));
    assert!(compile_error("let p = Point { x: \"three\" };").contains("declared"));
    assert!(compile_error("let p = Pont { x: 1 };").contains("no struct named Pont"));
}

#[test]
fn a_field_of_something_that_is_not_a_variable_is_refused() {
    let src = r#"
program P implements ISolve {
    public function f(): number { return 1; }
    @external
    public function solve(input: str): number {
        let a = f().x;
        return a;
    }
}
"#;
    // `let a = f().x;` used to read a register named "x" (silently 0)
    assert!(compiler::compile(src).is_err());
}
