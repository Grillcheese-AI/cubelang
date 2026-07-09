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
/// Three surface-syntax facts this program had to discover the hard way:
///   * `ctx` is a reserved TYPE keyword (`TyCtx`) -- the SPEC's
///     `let ctx: rag = knowledge.query(...)` made it one.
///   * `len` is not surface syntax. `op::LEN` is compiler-generated from `for`
///     and `let n = a.len()`. So this returns the array and Rust counts it.
///   * `VM::call` binds host arguments as `arg0, arg1, ...` -- NOT by the
///     declared parameter name. A `.cube` function that reads `mention` sees an
///     unbound register. That is a real ergonomic gap in host-side calls, noted
///     in `call()`'s own comment.
const PROGRAM: &str = r#"
program Ground {
    storage { queries: mutable u64 = 0; }

    @system @once
    public function constructor() { assign queries = 0; }

    @external
    public function solve(mention: str): quantity {
        query arg0;
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
