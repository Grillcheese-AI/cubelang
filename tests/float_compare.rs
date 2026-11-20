//! A decimal held in a register compares as a decimal (CubbyLLM 0078, 2026-11-19). The numeric COMPARE path used to
//! truncate both sides to i64: `0.678 <= 0.5` held and a register of 0.5 was falsy. Literal-to-literal comparisons are
//! folded at compile time, so each case here keeps its values in registers.
use cubelang::compiler;
use cubelang::vm::{ExecResult, Value, VM};

fn run(body: &str) -> i64 {
    let src = format!("program P implements ISolve {{
    public function solve(x: str): number {{
        {}
    }}
}}
", body);
    let mut progs = compiler::compile_strict(&src).unwrap_or_else(|e| panic!("compiles strict: {}
{}", e, src));
    let prog = progs.pop().unwrap();
    let name = prog.name.clone();
    let mut vm = VM::new();
    vm.load(prog);
    match vm.call(&name, "solve", vec![Value::Str("x".into())]) {
        ExecResult::Ok(v) | ExecResult::Return(v) => v.as_i64(),
        other => panic!("{:?}", other),
    }
}

#[test]
fn decimals_in_registers_compare_as_decimals() {
    let r = run("let y = 0.678; let x = 0.000001; let h = 0.5; let r = 0;\n\
                 if (y <= x) { r = r + 1; } if (y <= h) { r = r + 10; } if (y > x) { r = r + 100; }\n\
                 if (h < y) { r = r + 1000; } if (h == 0.5) { r = r + 10000; } return r;");
    assert_eq!(r, 11100);
}

#[test]
fn a_decimal_against_a_whole_number_and_truthiness() {
    let r = run("let h = 0.5; let one = 1; let r = 0;\n\
                 if (h < one) { r = r + 1; } if (one > h) { r = r + 10; } if (h) { r = r + 100; } return r;");
    assert_eq!(r, 111);
}

#[test]
fn whole_numbers_still_compare_exactly() {
    let r = run("let a = 9007199254740993; let b = 9007199254740992; let r = 0; if (a > b) { r = 1; } return r;");
    assert_eq!(r, 1);
}
