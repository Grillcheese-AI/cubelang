//! QUERY: exact grounding with provenance, and the three-valued contract.
//!
//! QUERY was `trace_structural` -- it read a register, logged a line, and
//! retrieved nothing, while the SPEC typed it as
//! `rag { chunks: array<{text, score, source}> }`.
//!
//! It now performs an exact hash lookup and pushes the chunk array. The count
//! drives the program:
//!     0 chunks -> gap    (say you don't know)
//!     1 chunk  -> fact   (render it)
//!     N chunks -> ASK    (only the caller knows which)
//!
//! Zero embeddings. A canonical title is a key.

use cubelang::compiler;
use cubelang::vm::{ExecResult, VM, Value};

const FACTS: &str = concat!(
    r#"{"key":"printing press","text":"Johannes Gutenberg, around 1440","source":"https://dbpedia.org/page/Printing_press"}"#, "\n",
    r#"{"alias":"movable type press","of":"printing press"}"#, "\n",
    r#"{"key":"mercury","text":"the innermost planet","source":"https://dbpedia.org/page/Mercury_(planet)"}"#, "\n",
    r#"{"key":"mercury","text":"chemical element, symbol Hg","source":"https://dbpedia.org/page/Mercury_(element)"}"#, "\n",
);

/// Ground a mention and hand the chunk array straight back.
///
/// Reads the DECLARED PARAMETER NAME `mention`. That used to be impossible:
/// `CompiledFunction` discarded parameter names, so `VM::call` synthesized
/// `arg0` and a program reading `mention` got an unbound register -- silently,
/// because unbound registers resolve to Int(0) rather than erroring. Every
/// example in the SPEC hit this. Fixed in .cubebin v3.
///
/// Two surface-syntax facts that remain, discovered here:
///   * `ctx` is a reserved TYPE keyword (`TyCtx`) -- the SPEC's own
///     `let ctx: rag = knowledge.query(...)` cannot be written.
///   * `len` is not surface syntax. `op::LEN` is compiler-generated from `for`
///     and `let n = a.len()`. So this returns the array and Rust counts it.
const PROGRAM: &str = r#"
program Ground implements ISolve {
    storage { queries: mutable u64 = 0; }

    @system @once
    public function constructor() { assign queries = 0; }

    @external
    public function solve(mention: str): number {
        query mention;
        create hits : number;
        pop hits;
        return hits;
    }
}
"#;

fn vm_with_facts() -> (VM, String) {
    let mut progs = compiler::compile(PROGRAM).expect("compile");
    let prog = progs.pop().expect("one program");
    let name = prog.name.clone();
    let mut vm = VM::new();
    let (facts, aliases) = vm.knowledge.load_jsonl(FACTS).expect("load facts");
    assert_eq!((facts, aliases), (3, 1));
    vm.load(prog);
    let _ = vm.call(&name, "constructor", vec![]);
    (vm, name)
}

/// Run `solve` and return the chunk array QUERY pushed.
fn chunks(vm: &mut VM, name: &str, mention: &str) -> Vec<Value> {
    match vm.call(name, "solve", vec![Value::Str(mention.to_string())]) {
        ExecResult::Ok(Value::Array(v)) | ExecResult::Return(Value::Array(v)) => v,
        other => panic!("QUERY must yield an Array, got {:?}", other),
    }
}

fn count(vm: &mut VM, name: &str, mention: &str) -> usize {
    chunks(vm, name, mention).len()
}

#[test]
fn query_on_an_empty_store_abstains() {
    let mut progs = compiler::compile(PROGRAM).unwrap();
    let prog = progs.pop().unwrap();
    let name = prog.name.clone();
    let mut vm = VM::new();
    vm.load(prog);
    let _ = vm.call(&name, "constructor", vec![]);
    // A VM with no knowledge abstains on everything. That is correct.
    assert_eq!(count(&mut vm, &name, "printing press"), 0);
}

#[test]
fn one_chunk_for_a_grounded_entity() {
    let (mut vm, name) = vm_with_facts();
    assert_eq!(count(&mut vm, &name, "printing press"), 1);
}

#[test]
fn aliases_ground_without_a_model() {
    // 'movable type press' shares no content word with 'printing press'.
    // An embedding model would be needed to relate them -- unless the alias
    // is simply another key, which is what a redirect table gives you free.
    let (mut vm, name) = vm_with_facts();
    assert_eq!(count(&mut vm, &name, "movable-type PRESS"), 1);
}

#[test]
fn REGRESSION_a_miss_returns_zero_chunks_not_a_neighbour() {
    // 'goldsmith' is topically adjacent to Gutenberg and appears in no key.
    // Returning a nearest neighbour here is how a fact store confabulates with
    // a citation attached. Zero chunks is the honest answer.
    let (mut vm, name) = vm_with_facts();
    assert_eq!(count(&mut vm, &name, "goldsmith"), 0);
    assert_eq!(count(&mut vm, &name, "xyzzy blorptangle"), 0);
}

#[test]
fn ambiguity_yields_many_chunks_so_the_program_can_ASK() {
    // 'Mercury' is a planet AND an element. The store holds both; it does not
    // choose. Two chunks is the cue to ASK -- the only honest move when only
    // the caller knows which was meant.
    let (mut vm, name) = vm_with_facts();
    assert_eq!(count(&mut vm, &name, "Mercury"), 2);
}

#[test]
fn every_chunk_carries_text_score_and_source() {
    let (mut vm, name) = vm_with_facts();
    let hits = chunks(&mut vm, &name, "printing press");
    assert_eq!(hits.len(), 1);

    let m = match &hits[0] {
        Value::Map(m) => m,
        other => panic!("chunk must be a Map, got {:?}", other),
    };

    match m.get("text") {
        Some(Value::Str(s)) => assert!(s.contains("Gutenberg"), "{}", s),
        other => panic!("bad text: {:?}", other),
    }
    match m.get("source") {
        // provenance is not optional
        Some(Value::Str(s)) => assert!(s.starts_with("https://"), "{}", s),
        other => panic!("bad source: {:?}", other),
    }
    match m.get("score") {
        // an exact key match is 1.0 -- not a similarity
        Some(Value::Float(f)) => assert!((f - 1.0).abs() < 1e-9),
        other => panic!("bad score: {:?}", other),
    }
}

#[test]
fn a_fact_without_a_source_is_rejected_at_load() {
    let mut vm = VM::new();
    let err = vm
        .knowledge
        .load_jsonl(r#"{"key":"printing press","text":"Gutenberg"}"#)
        .unwrap_err();
    assert!(err.contains("source"), "{}", err);
    assert!(vm.knowledge.is_empty());
}

// ── the host-call gap ─────────────────────────────────────────────────────
// `CompiledFunction` discarded declared parameter names, so `VM::call` had
// nothing to bind to and synthesized `arg0`. A `.cube` reading `mention` saw an
// unbound register -- which resolves to Int(0), not an error. Silent.

#[test]
fn REGRESSION_the_compiler_keeps_declared_parameter_names() {
    let progs = compiler::compile(PROGRAM).expect("compile");
    let solve = progs[0]
        .functions
        .iter()
        .find(|f| f.name == "solve")
        .expect("solve");
    assert_eq!(solve.params, vec!["mention".to_string()]);

    // and a zero-arg function has none
    let ctor = progs[0].functions.iter().find(|f| f.name == "constructor").unwrap();
    assert!(ctor.params.is_empty());
}

#[test]
fn REGRESSION_host_args_bind_to_the_declared_name() {
    // The program reads `mention`, not `arg0`. If binding regressed, the lookup
    // silently misses and this returns zero chunks.
    let (mut vm, name) = vm_with_facts();
    assert_eq!(count(&mut vm, &name, "printing press"), 1);
}

#[test]
fn argN_alias_still_works_for_backwards_compatibility() {
    const BY_ARGN: &str = r#"
program ByArgN implements ISolve {
    @external
    public function solve(mention: str): number {
        query arg0;
        create hits : number;
        pop hits;
        return hits;
    }
}
"#;
    let mut progs = compiler::compile(BY_ARGN).expect("compile");
    let prog = progs.pop().unwrap();
    let name = prog.name.clone();
    let mut vm = VM::new();
    vm.knowledge.load_jsonl(FACTS).unwrap();
    vm.load(prog);
    match vm.call(&name, "solve", vec![Value::Str("printing press".into())]) {
        ExecResult::Ok(Value::Array(v)) | ExecResult::Return(Value::Array(v)) => {
            assert_eq!(v.len(), 1)
        }
        other => panic!("argN alias broke: {:?}", other),
    }
}

#[test]
fn params_survive_the_cubebin_round_trip() {
    // v3 carries parameter names between the function name and its bytecode.
    // Without this, `cubelang run prog.cubebin` would lose them and fall back
    // to `arg0` -- the bug would return the moment you compiled to disk.
    let progs = compiler::compile(PROGRAM).expect("compile");
    let bytes = progs[0].to_cubebin();
    let back = compiler::CompiledProgram::from_cubebin(&bytes).expect("round trip");

    let solve = back.functions.iter().find(|f| f.name == "solve").unwrap();
    assert_eq!(solve.params, vec!["mention".to_string()]);

    // and the reconstructed program still grounds
    let name = back.name.clone();
    let mut vm = VM::new();
    vm.knowledge.load_jsonl(FACTS).unwrap();
    vm.load(back);
    let _ = vm.call(&name, "constructor", vec![]);
    match vm.call(&name, "solve", vec![Value::Str("Movable Type Press".into())]) {
        ExecResult::Ok(Value::Array(v)) | ExecResult::Return(Value::Array(v)) => {
            assert_eq!(v.len(), 1, "alias lookup after round trip")
        }
        other => panic!("round-tripped program failed: {:?}", other),
    }
}

// ── QUERY -> ASK: the composition, and the bugs it exposed ────────────────

/// The whole three-valued contract, in one program. `ctx` is the SPEC's own
/// name for the RAG context -- and was a reserved TYPE keyword, so the SPEC's
/// example could not be written until type keywords became soft.
const GROUND: &str = r#"
program Ground implements ISolve {
    @external
    public function solve(mention: str): number {
        query mention;
        create ctx : number;
        pop ctx;
        let n = ctx.len();
        create outcome : number;
        assign outcome = 0;
        if (n == 0) {
            assign outcome = 0;
        } else {
            if (n == 1) {
                assign outcome = 1;
            } else {
                ask "which one did you mean", ctx;
                assign outcome = 2;
            }
        }
        return outcome;
    }
}
"#;

fn ground_vm() -> (VM, String) {
    let mut progs = compiler::compile(GROUND).expect("compile GROUND");
    let prog = progs.pop().unwrap();
    let name = prog.name.clone();
    let mut vm = VM::new();
    vm.knowledge.load_jsonl(FACTS).unwrap();
    vm.load(prog);
    (vm, name)
}

#[test]
fn REGRESSION_type_keywords_are_usable_as_identifiers() {
    // `ctx`, `map`, `role`, `file`, `url`, `request`, `set` ... are ordinary
    // words. Reserving them outright made the SPEC's own example unwritable:
    //     let ctx: rag = knowledge.query("...", top_k: 5);
    const NAMES: &str = r#"
program Names implements ISolve {
    @external
    public function solve(input: str): number {
        create ctx : number;   assign ctx = 1;
        create map : number;   assign map = 2;
        create role : number;  assign role = 3;
        create request : number; assign request = 4;
        return ctx;
    }
}
"#;
    let progs = compiler::compile(NAMES).expect("type keywords must parse as identifiers");
    assert_eq!(progs[0].name, "Names");
}

#[test]
fn zero_chunks_is_a_gap_one_is_a_fact() {
    let (mut vm, name) = ground_vm();
    for (mention, expect) in [
        ("goldsmith", 0i64),          // no such key -> gap
        ("xyzzy blorptangle", 0),     // nonsense -> gap
        ("printing press", 1),        // exactly one grounded fact
        ("Movable-Type PRESS", 1),    // same fact, reached by alias
    ] {
        match vm.call(&name, "solve", vec![Value::Str(mention.into())]) {
            ExecResult::Ok(Value::Int(n)) | ExecResult::Return(Value::Int(n)) => {
                assert_eq!(n, expect, "{}", mention)
            }
            other => panic!("{}: {:?}", mention, other),
        }
    }
}

#[test]
fn REGRESSION_ask_flattens_the_chunk_array_into_candidates() {
    // `ask "...", ctx;` where ctx is the array QUERY pushed means "choose among
    // these". Passing it as ONE candidate that happens to be a list makes the
    // selection unnameable -- and resume's guard then rejects every answer.
    let (mut vm, name) = ground_vm();
    let susp = match vm.call(&name, "solve", vec![Value::Str("Mercury".into())]) {
        ExecResult::Suspend(s) => s,
        other => panic!("expected Suspend on an ambiguous entity, got {:?}", other),
    };

    assert_eq!(susp.candidates.len(), 2, "candidates must be flat, not nested");
    for c in &susp.candidates {
        match c {
            Value::Map(m) => {
                assert!(matches!(m.get("source"), Some(Value::Str(s)) if s.starts_with("http")));
                assert!(m.contains_key("text"));
            }
            other => panic!("each candidate must be a chunk Map, got {:?}", other),
        }
    }
}

#[test]
fn REGRESSION_the_ask_question_text_survives_compilation() {
    // String literals compile to a 2-byte hash. Unless the spelling is recorded
    // in symbol_names, the question comes back as `g_e009` -- and the question
    // is the very thing the trunk is supposed to render into language.
    let (mut vm, name) = ground_vm();
    let susp = match vm.call(&name, "solve", vec![Value::Str("Mercury".into())]) {
        ExecResult::Suspend(s) => s,
        other => panic!("{:?}", other),
    };
    match &susp.question {
        Value::Str(q) => assert_eq!(q, "which one did you mean"),
        other => panic!("question must be its literal text, got {:?}", other),
    }
}

#[test]
fn REGRESSION_resume_accepts_a_chunk_Map_as_a_selection() {
    // resume's guard compared scalars only. A grounded candidate is a Map, so
    // the guard would have rejected EVERY possible answer -- a safety check
    // that made the feature impossible to use.
    let (mut vm, name) = ground_vm();
    let susp = match vm.call(&name, "solve", vec![Value::Str("Mercury".into())]) {
        ExecResult::Suspend(s) => s,
        other => panic!("{:?}", other),
    };
    let planet = susp.candidates[0].clone();
    match vm.resume(susp, planet) {
        ExecResult::Ok(Value::Int(n)) | ExecResult::Return(Value::Int(n)) => assert_eq!(n, 2),
        other => panic!("resume must accept an offered chunk, got {:?}", other),
    }
}

#[test]
fn resume_still_rejects_an_invented_chunk() {
    // The guarantee survives the structural comparison: a Map that was never
    // offered is a fabrication, however well-formed it looks.
    use std::collections::HashMap;
    let (mut vm, name) = ground_vm();
    let susp = match vm.call(&name, "solve", vec![Value::Str("Mercury".into())]) {
        ExecResult::Suspend(s) => s,
        other => panic!("{:?}", other),
    };

    let mut fake = HashMap::new();
    fake.insert("text".to_string(), Value::Str("a Roman god".into()));
    fake.insert("score".to_string(), Value::Float(1.0));
    fake.insert("source".to_string(), Value::Str("https://example.invalid".into()));

    match vm.resume(susp, Value::Map(fake)) {
        ExecResult::Error(e) => assert!(e.contains("chosen, not invented"), "{}", e),
        other => panic!("a fabricated chunk must be rejected, got {:?}", other),
    }
}
