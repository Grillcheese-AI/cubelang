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
program Ground {
    storage { queries: mutable u64 = 0; }

    @system @once
    public function constructor() { assign queries = 0; }

    @external
    public function solve(mention: str): quantity {
        query mention;
        create hits : quantity;
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
program ByArgN {
    @external
    public function solve(mention: str): quantity {
        query arg0;
        create hits : quantity;
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
