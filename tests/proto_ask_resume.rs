//! ASK / resume over `run-proto` (2026-08-30).
//!
//! `run-proto` is one-shot (one RunRequest in, one RunResult out, process
//! exits), so a suspension cannot be held open. The contract instead is
//! resume-by-re-execution: the first request reports `Suspended{question,
//! candidates}`; the caller re-sends the same request with the chosen
//! candidate (JSON) appended to `answers`; the program re-executes, reaches
//! the same ASK, and `VM::resume` consumes the answer -- keeping its
//! guarantee that an answer must be one of the offered candidates.
use cubelang::reasoning::{RunRequest, RunResult, run_result};
use prost::Message;
use std::io::{Read, Write};
use std::process::{Command, Stdio};

fn call_proto(req: RunRequest) -> RunResult {
    let exe = env!("CARGO_BIN_EXE_cubelang");
    let mut child = Command::new(exe).arg("run-proto")
        .stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut buf = Vec::new(); req.encode(&mut buf).unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(&(buf.len() as u32).to_be_bytes()).unwrap();
    stdin.write_all(&buf).unwrap();
    drop(stdin);
    let mut out = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut out).unwrap();
    child.wait().unwrap();
    let len = u32::from_be_bytes(out[..4].try_into().unwrap()) as usize;
    RunResult::decode(&out[4..4 + len]).unwrap()
}

/// examples/ask_min.cube, verbatim in spirit: two grounded candidates, no way
/// to choose, ASK, resume with the caller's selection.
const ASK_PROGRAM: &str = r#"
program AskMin implements ISolve {
    storage { asked: mutable u64 = 0; }

    @system @once
    public function constructor() { assign asked = 0; }

    @external
    public function solve(input: str): number {
        create luna : number;
        assign luna = 1959;
        create apollo : number;
        assign apollo = 1969;
        ask "which moon landing did you mean", luna, apollo;
        create chosen : number;
        pop chosen;
        sum chosen;
        return chosen;
    }
}
"#;

fn req(answers: Vec<&str>) -> RunRequest {
    RunRequest {
        program: ASK_PROGRAM.into(),
        args: vec!["_".into()],
        fn_name: "solve".into(),
        answers: answers.into_iter().map(String::from).collect(),
    }
}

#[test]
fn ask_over_proto_reports_a_suspension_with_both_candidates() {
    let res = call_proto(req(vec![]));
    assert!(res.ok, "a suspension is not a failure");
    match res.result {
        Some(run_result::Result::Suspended(s)) => {
            assert_eq!(s.question, "\"which moon landing did you mean\"");
            assert_eq!(s.candidates, vec!["1959".to_string(), "1969".to_string()]);
            assert_eq!(s.function, "solve");
            assert_eq!(s.program, "AskMin");
        }
        other => panic!("expected Suspended, got {:?}", other),
    }
    assert!(res.similarity.is_none(), "no recover() ran; no stale similarity");
}

#[test]
fn resume_over_proto_by_reexecution_returns_the_chosen_candidate() {
    let res = call_proto(req(vec!["1969"]));
    assert!(res.ok);
    assert!(matches!(res.result, Some(run_result::Result::Symbol(ref s)) if s == "1969"), "got {:?}", res.result);
    let res = call_proto(req(vec!["1959"]));
    assert!(matches!(res.result, Some(run_result::Result::Symbol(ref s)) if s == "1959"), "got {:?}", res.result);
}

#[test]
fn resume_over_proto_rejects_an_invented_answer() {
    // THE GUARANTEE survives the wire: the caller may only choose, never supply.
    let res = call_proto(req(vec!["1968"]));
    assert!(!res.ok);
    match res.result {
        Some(run_result::Result::Error(e)) => {
            assert!(e.contains("not among"), "unexpected error text: {}", e);
        }
        other => panic!("a fabricated selection must be rejected, got {:?}", other),
    }
}

#[test]
fn resume_over_proto_rejects_non_json_and_hypervector_answers() {
    let res = call_proto(req(vec!["not json at all"]));
    assert!(!res.ok);
    assert!(matches!(res.result, Some(run_result::Result::Error(ref e)) if e.contains("not JSON")), "got {:?}", res.result);
    let res = call_proto(req(vec!["{\"hvec_dim\": 4096}"]));
    assert!(!res.ok);
    assert!(matches!(res.result, Some(run_result::Result::Error(ref e)) if e.contains("hypervector")), "got {:?}", res.result);
}

#[test]
fn non_asking_programs_are_unaffected_by_answers() {
    // A program that never ASKs ignores `answers` entirely.
    let program = r#"
use vsa;
program R implements ISolve {
    public function solve(input: str): str {
        create frame: number;
        bind frame, SUBJECT, "cat";
        return recover(frame, SUBJECT);
    }
}
"#;
    let res = call_proto(RunRequest { program: program.into(), args: vec!["_".into()], fn_name: "solve".into(),
                                      answers: vec!["\"ignored\"".into()] });
    assert!(res.ok);
    assert!(matches!(res.result, Some(run_result::Result::Symbol(ref s)) if s == "cat"));
}
