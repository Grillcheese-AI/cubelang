# ASK / Suspend — the third execution outcome

**Status: implemented, tested (6 tests in `tests/ask_suspend.rs`), 141 green.**

## The problem

`ExecResult` was `Ok | Return | Error`. That is a two-valued world with an error
channel: a program either produced an answer or failed.

But a grounded reasoning program has a third situation. Consider the measured
DBpedia case: the `Moon landing` abstract cites **both** Luna 2 (13 September
1959) and Apollo 11 (20 July 1969). Asked *"when did the moon landing happen?"*
the program has:

- linked the right entity,
- extracted two real, cited candidates,
- and no way to choose between them.

It is **not ignorant** — it knows two things. It has **not failed** — nothing
went wrong. The one thing it lacks is *which one you meant*, and that is not
derivable from any corpus. Only the caller has it.

With two outcomes, such a program must either pick one (a **cited falsehood** —
"the moon landing was in 1959, source: dbpedia.org/Moon_landing") or return
`Error` (a lie: nothing failed). Both are wrong.

## The change

```rust
pub enum ExecResult {
    Ok(Value),
    Return(Value),
    Suspend(Suspension),   // NEW
    Error(String),
}
```

`ASK` was a keyword — lexed, parsed, allocated an opcode byte, dispatched — and
its handler was `self.trace_structural(pc, "ASK", &operands)`. It recorded that
an ask happened and did nothing. It is now an instruction:

```
ask "which moon landing did you mean", luna, apollo;
```

produces `ExecResult::Suspend`, carrying the question, the candidates, and
everything needed to continue.

### Why resumption is cheap

Registers, stack, storage, memory and the accumulator are all **VM-global** —
the flat model that made `CALL` awkward makes `Suspend` easy. Only the
interpreter's *locals* need capturing:

```rust
pub struct Suspension {
    pub question: Value,
    pub candidates: Vec<Value>,
    pub pc: usize,          // already points PAST the ASK
    pub flag: bool,         // COMPARE's result, or a following COND misbehaves
    pub jump_budget: u64,
    pub function: String,
    pub program: String,    // call() restores current_program on the way out
}
```

`VM::resume(susp, answer)` restores those, pushes the answer onto the stack, and
re-enters. No call frames, no continuation capture.

### The answer arrives on the stack, not the accumulator

An earlier draft put the answer in the accumulator. That was wrong: `SUM x`
copies *register x → accumulator*, so the next `sum` clobbered it. `CALL` already
delivers a callee's return value via `stack.push(v)` + a compiler-emitted `POP`.
`resume` follows the same convention:

```
ask "...", luna, apollo;   # suspends
create chosen : number;
pop chosen;                # the answer
sum chosen;
return chosen;
```

## THE CALLER IS THE MODEL, NOT A HUMAN

The VM never talks to a person. The loop is:

```
VM       ASK(question, [1959, 1969])  ->  Suspend
Trunk    renders it as language:
         "Do you mean the first object to reach the Moon (Luna 2, 1959),
          or the first crewed landing (Apollo 11, 1969)?"
User     "the crewed one"                    <- natural language, never an index
Trunk    maps that back to candidates[1]
VM       resume(susp, 1969)                  ->  continues
```

The trunk does **rendering** on the way out and **selection** on the way back.
Both are safe jobs: it supplies *syntax* and *choice*, never *content*.

### And the selection is checked

The trunk is the confabulating component in this system. (At trunk step 101k it
still writes that the printing press "began to use individual characters" in
1638.) But here it can only ever **select**, never **supply** — and a selection
is checkable by `==`, not by judgement.

`resume` enforces it:

```rust
if !susp.candidates.iter().any(|c| same_selection(c, &answer)) {
    return ExecResult::Error(
        "resume: answer {} is not among the {} candidates the program offered \
         (a selection must be chosen, not invented)"
    );
}
```

A trunk that returns `1968` for candidates `{1959, 1969}` has hallucinated a
value into a slot that could only ever hold one of two. The VM rejects it
structurally rather than believing it. This is the same discipline as verifying
that a rendered sentence contains the extracted span: **the generative step is
permitted precisely because it is constrained and verified.**

`same_selection` compares Int/Str/Bool/Float only. `Value` deliberately does not
`derive(PartialEq)` — it holds `Hvec`, and "equal hypervectors" is a similarity
question, not an equality one.

## Honest limitations

**`ASK` inside a `CALL` is rejected.** By the time a callee's suspension reaches
the `CALL` arm, the caller's registers have been restored and `call_depth`
unwound; resuming would re-enter the callee with the wrong frame. It fails loudly
rather than corrupting state:

> `ASK inside a CALL is not supported (in {fn}): the flat global-register model
> cannot capture a callee frame.`

Lifting this needs the real call frames that `CUBELANG_OPCODES_PLAN.md` already
defers to v2. `ASK` at the top level of `solve` works — which is where a
disambiguation belongs anyway.

**The question text does survive compilation.** String literals compile to a
2-byte hash (`Operand::Global`), but `emit_global` records the real spelling
into `symbol_names` at compile time (`record_symbol`), and `load()` copies
`symbol_names` into the VM's name table. `ASK`'s question operand resolves via
`resolve_symbol_name`, which reverses the hash back to the literal's real
text — so `question` comes back as `"which moon landing did you mean"`, the
actual sentence, not `"g_727b"`. Candidates were always unaffected (real
values). The `g_xxxx` token now surfaces only for a genuinely unrecorded
literal — deterministic, just not human-readable — which is not the normal
case.

**`cubelang run` without `--json` is a debug harness**, not the interface. It
prints numbered options and reads an index from stdin so the contract can be
exercised from a terminal. It announces itself on stderr. `--json` is the real
host boundary:

```json
{"suspended":true,"program":"AskMin","function":"solve",
 "question":"which moon landing did you mean","candidates":[1959,1969]}
```

## Where this sits

| grounding step | opcode | status |
|---|---|---|
| link entity | `QUERY` | **trace-only** — no retrieval |
| fetch abstract | `RECALL` | executes (hippocampal memory) |
| count candidates | `LEN` | executes |
| branch 0 / 1 / many | `COMPARE` + `COND` | executes |
| gap -> return | `RETURN` | executes |
| one -> return | `INDEX` + `RETURN` | executes |
| **many -> ask** | **`ASK`** | **executes (this change)** |

Every internal step works. The two that cross the VM's boundary were stubs:
*retrieve* and *ask*. One is done.

`QUERY` is next. Its SPEC already types it as
`rag { chunks: array<{text, score, source}> }` — provenance in the type. Wiring
it to the DBpedia title index (4.6M entities, 390 µs/query, zero encoders) turns
the three-valued Python `answer_when()` into a `.cube` program: `QUERY` the
entity, `LEN` the spans, `COND` on the count, then `RETURN` a gap, `RETURN` a
fact, or `ASK` the candidates.
