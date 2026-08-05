//! CubeLang `validate` subcommand (P0-3).
//!
//! Walks an AST and reports — per function — every construct that COMPILES
//! cleanly but does NOT actually execute under the current VM, so a caller can
//! confirm at a glance that a program is fully executable before shipping it.
//!
//! Findings (each carries source line when available):
//!   * Trace-only extended opcodes (predict / score / infer / ...)        — see
//!     [`compiler::is_trace_only_ext_op`] for the authoritative list.
//!   * `unify` (trace-only in the VM).
//!   * `match` — every arm body compiles unconditionally (P2-2).
//!
//! NOTE: `return` no longer appears here — it now executes as a real
//! early-exit opcode (Plan-1 / op::RETURN, byte 0x39).
//! NOTE: `call` no longer appears here — intra-program CALL now executes
//! (Plan-2): registers snapshot/restore + stack-delivered return.
//! NOTE: `for` no longer appears here — `for x of arr` lowers to a real
//! index-based while loop using LEN/INDEX (Plan-3 + Plan-4).
//!
//! `validate examples/qc_decision.cube` ⇒ "all executing, no stubs".

use crate::ast::*;
use crate::compiler::{ext_name, is_trace_only_ext_op};

/// One specific stub or trace-only construct detected in a function body.
#[derive(Debug, Clone)]
pub enum Finding {
    /// `match (expr) { arms }` — every arm body executes unconditionally.
    Match { line: u32 },
    /// Trace-only extended opcode (`predict`, `infer`, `score`, ...).
    TraceOnlyOpcode { name: &'static str },
    /// `unify a, b` — trace-only in the current VM.
    UnifyOpcode,
}

impl std::fmt::Display for Finding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Finding::Match { line } =>
                write!(f, "match at line {} (arms execute unconditionally — P2-2)", line),
            Finding::TraceOnlyOpcode { name } =>
                write!(f, "opcode `{}` (trace-only)", name),
            Finding::UnifyOpcode =>
                write!(f, "opcode `unify` (trace-only)"),
        }
    }
}

/// Per-function report. `findings` is empty when every construct executes.
#[derive(Debug, Clone)]
pub struct FunctionReport {
    pub name: String,
    pub findings: Vec<Finding>,
}

/// Per-program report.
#[derive(Debug, Clone)]
pub struct ProgramReport {
    pub name: String,
    pub functions: Vec<FunctionReport>,
}

impl ProgramReport {
    pub fn is_clean(&self) -> bool {
        self.functions.iter().all(|f| f.findings.is_empty())
    }
}

/// Full validation report for a source file.
#[derive(Debug, Clone)]
pub struct ValidationReport {
    pub programs: Vec<ProgramReport>,
}

impl ValidationReport {
    pub fn is_clean(&self) -> bool {
        self.programs.iter().all(|p| p.is_clean())
    }

    pub fn finding_count(&self) -> usize {
        self.programs.iter()
            .flat_map(|p| p.functions.iter())
            .map(|f| f.findings.len())
            .sum()
    }
}

impl std::fmt::Display for ValidationReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for prog in &self.programs {
            writeln!(f, "program {}", prog.name)?;
            for func in &prog.functions {
                if func.findings.is_empty() {
                    writeln!(f, "  {}(): all executing", func.name)?;
                } else {
                    writeln!(f, "  {}(): {} trace-only / stub construct(s)",
                        func.name, func.findings.len())?;
                    for finding in &func.findings {
                        writeln!(f, "    - {}", finding)?;
                    }
                }
            }
            if prog.is_clean() {
                writeln!(f, "  ✓ all executing, no stubs")?;
            }
        }
        Ok(())
    }
}

/// Parse + validate. The Err only carries parse failures — validation findings
/// always come back inside Ok(report); callers decide whether findings are an
/// error (exit 1) or informational.
pub fn validate(source: &str) -> Result<ValidationReport, String> {
    let ast = crate::parser::parse(source).map_err(|e| format!("parse error: {}", e))?;
    let mut programs = Vec::new();
    for item in &ast.items {
        if let TopLevel::Program(p) = item {
            programs.push(validate_program(p));
        }
    }
    Ok(ValidationReport { programs })
}

fn validate_program(prog: &ProgramDecl) -> ProgramReport {
    let mut functions = Vec::new();
    for item in &prog.body {
        match item {
            ProgramItem::Function(f) | ProgramItem::Constructor(f) => {
                functions.push(validate_function(f));
            }
            _ => {}
        }
    }
    ProgramReport { name: prog.name.clone(), functions }
}

fn validate_function(func: &FunctionDecl) -> FunctionReport {
    let mut findings = Vec::new();
    for stmt in &func.body {
        walk_stmt(stmt, &mut findings);
    }
    FunctionReport { name: func.sig.name.clone(), findings }
}

fn walk_stmt(stmt: &Stmt, findings: &mut Vec<Finding>) {
    match stmt {
        Stmt::For(f) => {
            for s in &f.body { walk_stmt(s, findings); }
        }
        Stmt::Match(m) => {
            findings.push(Finding::Match { line: m.span.line });
            for arm in &m.arms {
                for s in &arm.body { walk_stmt(s, findings); }
            }
        }
        Stmt::While(w) => {
            for s in &w.body { walk_stmt(s, findings); }
        }
        Stmt::If(i) => {
            for s in &i.then_body { walk_stmt(s, findings); }
            if let Some(else_body) = &i.else_body {
                for s in else_body { walk_stmt(s, findings); }
            }
        }
        Stmt::Atomic(a) => {
            for s in &a.body { walk_stmt(s, findings); }
        }
        Stmt::TryCatch(tc) => {
            for s in &tc.try_body  { walk_stmt(s, findings); }
            for s in &tc.catch_body { walk_stmt(s, findings); }
        }
        // Plan-2: intra-program CALL now executes — `Stmt::Expr(Call/MethodCall)`
        // is no longer a stub finding.
        Stmt::Opcode(OpcodeStmt::Extended { op, .. }) => {
            if is_trace_only_ext_op(*op) {
                findings.push(Finding::TraceOnlyOpcode { name: ext_name(*op) });
            }
        }
        Stmt::Opcode(OpcodeStmt::Unify { .. }) => {
            findings.push(Finding::UnifyOpcode);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_qc_decision_is_clean() {
        let source = include_str!("../examples/qc_decision.cube");
        let report = validate(source).expect("qc_decision.cube must parse");
        assert!(report.is_clean(),
            "qc_decision.cube should be 'all executing, no stubs'; got: {}",
            report);
        // Sanity: at least one program was reported.
        assert_eq!(report.programs.len(), 1);
        assert_eq!(report.programs[0].name, "QCDecision");
    }

    #[test]
    fn validate_flags_predict_with_name() {
        let source = r#"
            program Test implements ISolver {
                public function run(): void {
                    predict x;
                }
            }
        "#;
        let report = validate(source).unwrap();
        assert!(!report.is_clean());
        let printed = format!("{}", report);
        assert!(printed.contains("predict"), "report should name 'predict': {}", printed);
    }

    #[test]
    fn validate_for_is_not_flagged() {
        // Plan-4 made `for` a real index-based loop; it is no longer a stub.
        let source = "
            program Test implements ISolver {
                public function run(): void {
                    let xs = [1, 2, 3];
                    create total : number;
                    assign total = 0;
                    for (let x of xs) {
                        add total, x;
                    }
                    sum total;
                }
            }
        ";
        let report = validate(source).unwrap();
        assert!(report.is_clean(), "real for-each should not be flagged; got: {}", report);
    }

    #[test]
    fn validate_flags_match_with_line() {
        let source = "
            program Test implements ISolver {
                public function run(): void {
                    match (x) {
                        1 => { sum x; }
                        _ => { query x; }
                    }
                }
            }
        ";
        let report = validate(source).unwrap();
        assert!(!report.is_clean());
        let printed = format!("{}", report);
        assert!(printed.contains("match"));
        assert!(printed.contains("line"));
    }

    #[test]
    fn validate_return_is_never_flagged() {
        // Plan-1 made `return` a real opcode (0x39), so neither trailing
        // returns nor early returns inside an if are stubs any more.
        let source = r#"
            program Test implements ISolver {
                public function solve(input: Input): Output {
                    create x : number;
                    assign x = 5;
                    if (x > 0) {
                        return x;
                    }
                    sum x;
                    return x;
                }
            }
        "#;
        let report = validate(source).unwrap();
        assert!(report.is_clean(),
            "real RETURN should not be flagged; got: {}", report);
    }

    #[test]
    fn validate_call_is_not_flagged() {
        // Plan-2: intra-program CALL executes for real, so call statements
        // are no longer stubs.
        let source = r#"
            program Test implements ISolver {
                public function helper(): number {
                    return 1;
                }
                public function run(): void {
                    helper();
                }
            }
        "#;
        let report = validate(source).unwrap();
        assert!(report.is_clean(), "call should not be flagged any more; got: {}", report);
    }
}
