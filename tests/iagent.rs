//! `IAgent` (2026-08-30): the SPEC's conversational/agent contract, now a
//! registry-seeded (VM-internal, tamper-proof) interface like `ISolver`.
//! Required members: `think(input, ctx)`, `act(decision, ctx)`,
//! `observe(result, ctx)` -- the spec's `plan`/`reflect` are optional there
//! and therefore not required here.
use cubelang::compiler;

const AGENT_OK: &str = r#"
program Talker implements IAgent {
    storage { turns: mutable u64 = 0; }

    @system @once
    public function constructor() { assign turns = 0; }

    public function think(input: str, ctx: str): str {
        add turns, 1;
        return input;
    }
    public function act(decision: str, ctx: str): str {
        return decision;
    }
    public function observe(result: str, ctx: str): str {
        return result;
    }
}
"#;

const AGENT_MISSING_OBSERVE: &str = r#"
program Talker implements IAgent {
    public function think(input: str, ctx: str): str { return input; }
    public function act(decision: str, ctx: str): str { return decision; }
}
"#;

const AGENT_WRONG_ARITY: &str = r#"
program Talker implements IAgent {
    public function think(input: str): str { return input; }
    public function act(decision: str, ctx: str): str { return decision; }
    public function observe(result: str, ctx: str): str { return result; }
}
"#;

#[test]
fn iagent_with_think_act_observe_compiles_strict() {
    let progs = compiler::compile_strict(AGENT_OK).expect("a conforming IAgent must compile");
    assert_eq!(progs.len(), 1);
    assert!(progs[0].implements.iter().any(|i| i == "IAgent"));
}

#[test]
fn iagent_missing_observe_is_a_compile_error_naming_the_member() {
    let err = compiler::compile_strict(AGENT_MISSING_OBSERVE).err().expect("must fail");
    let msg = err.to_string();
    assert!(msg.contains("IAgent"), "{}", msg);
    assert!(msg.contains("observe"), "{}", msg);
}

#[test]
fn iagent_think_with_wrong_arity_is_a_compile_error() {
    let err = compiler::compile_strict(AGENT_WRONG_ARITY).err().expect("must fail");
    let msg = err.to_string();
    assert!(msg.contains("think"), "{}", msg);
}

#[test]
fn a_program_cannot_redefine_iagent_inline() {
    // registry-first resolution: an inline `interface IAgent` with a weaker
    // shape must not win over the seeded contract.
    let src = r#"
interface IAgent {
    abstract public function think(input: str): str;
}
program Talker implements IAgent {
    public function think(input: str): str { return input; }
}
"#;
    assert!(compiler::compile_strict(src).is_err(), "the inline decl must not override the registry's IAgent");
}
