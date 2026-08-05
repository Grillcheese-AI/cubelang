//! VM-internal interface registry — the *contracts* half of Task 4 of the
//! CubeLang foundation cycle. Sibling to Task 3's `vm::registry::ModuleRegistry`
//! (callable native impls), but deliberately a SEPARATE type: an interface is
//! a contract a `.cube` program claims to satisfy (`implements ISolver`), not
//! something callable in its own right, so it doesn't belong inside
//! `ModuleRegistry`'s `module -> {fn -> NativeFn}` shape at all — there is no
//! `call` here, just a name and an arity per required member.
//!
//! Standard interfaces used to be pure decoration: `implements ISolver` parsed
//! (`ast::ProgramDecl::implements`) and round-tripped through `.cubebin`, but
//! nothing ever checked a program actually provided what it claimed. Making
//! the CONTRACT itself VM-internal (Rust source, not `.cube` source) is what
//! makes it tamper-proof — a program can declare its own inline `interface`
//! block with any name it likes, but it can never redefine what `ISolver`
//! (or `ISolve`) means; see `compiler::Compiler::check_implements` for the
//! registry-first resolution order that enforces this.
//!
//! `__wiring__`: WIRED. `Compiler::check_implements` builds a fresh
//! `InterfaceRegistry` for every `compile()`/`compile_strict()` call and
//! resolves every `implements` name against it before falling back to an
//! in-file inline `interface` decl.
//!
//! Three interfaces are seeded: `ISolve` (one abstract fn, `solve`), `ISolver`
//! (three: `parse`, `solve`, `verify` — the shape every one of this project's
//! `ISolver`-implementing examples already hand-declares inline; Task 7
//! deletes those six duplicate inline decls now that `implements` is
//! required and every program resolves against this registry instead), and
//! `ISolverLearn` (Task 7: `ISolver`'s triad plus a fourth REQUIRED member,
//! `learn` — for the two examples, `gsm8k.cube` and `conversation_agent.cube`,
//! that were each carrying their own inline interface with `parse`/`solve`/
//! `verify` abstract and `learn` merely `optional`. A registry interface has
//! no optional-member concept (see `InterfaceFn`'s doc), so this is a
//! DELIBERATELY stronger contract than either file's old inline one: both
//! programs already provide a real, non-stub `learn`, so requiring it costs
//! nothing here and is more honest than "optional" for two programs that
//! never actually omit it. A program that genuinely wants parse/solve/verify
//! without committing to learn still has plain `ISolver` available.

use std::collections::HashMap;

/// One abstract member an interface requires: just enough to validate a
/// program's `implements X` claim at compile time without needing full type
/// information — the member's name and its arity (parameter count). A
/// registry interface has no notion of an `optional` member (unlike an
/// in-file inline `interface` decl, e.g. gsm8k.cube's `optional ... learn`) —
/// every member listed here is required. `ISolve`/`ISolver` are both
/// all-abstract per the brief; if a future registry interface ever needs an
/// optional member, this shape will need a `required: bool` field to match
/// `compiler::Compiler::check_implements`'s inline-decl filter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceFn {
    pub name: String,
    pub arity: usize,
}

/// One interface's required surface: every member a conforming
/// `implements X` program must provide.
struct Interface {
    members: Vec<InterfaceFn>,
}

/// The VM-internal, name-addressed interface registry: interface name ->
/// its required members. Unlike `ModuleRegistry::resolve`, there is no
/// deny-by-default `use`-gating here — `implements X` resolves DIRECTLY
/// against this registry (Task 4's decision, superseding SPEC.md's
/// `use isolver;` shorthand): interfaces are *implemented*, not *called*, so
/// they never go through Task 3's `use`/call-resolution machinery.
pub struct InterfaceRegistry {
    interfaces: HashMap<String, Interface>,
}

impl InterfaceRegistry {
    /// Build the registry seeded with every interface the VM ships. Cheap
    /// and deterministic, like `ModuleRegistry::new()` — callers each build
    /// their own copy rather than sharing one instance; there is no dynamic
    /// registration in this task.
    pub fn new() -> Self {
        let mut interfaces = HashMap::new();
        interfaces.insert("ISolve".to_string(), Interface {
            members: vec![
                InterfaceFn { name: "solve".to_string(), arity: 1 },
            ],
        });
        interfaces.insert("ISolver".to_string(), Interface {
            members: vec![
                InterfaceFn { name: "parse".to_string(), arity: 1 },
                InterfaceFn { name: "solve".to_string(), arity: 1 },
                InterfaceFn { name: "verify".to_string(), arity: 2 },
            ],
        });
        interfaces.insert("ISolverLearn".to_string(), Interface {
            members: vec![
                InterfaceFn { name: "parse".to_string(), arity: 1 },
                InterfaceFn { name: "solve".to_string(), arity: 1 },
                InterfaceFn { name: "verify".to_string(), arity: 2 },
                InterfaceFn { name: "learn".to_string(), arity: 3 },
            ],
        });
        Self { interfaces }
    }

    /// Every required (name, arity) member for `name`, if the registry knows
    /// an interface by that name. `None` means "not a VM-internal interface"
    /// — the caller (`Compiler::check_implements`) then falls back to an
    /// in-file inline `interface` decl before deciding `implements` is
    /// unresolvable.
    pub fn get(&self, name: &str) -> Option<&[InterfaceFn]> {
        self.interfaces.get(name).map(|i| i.members.as_slice())
    }
}

impl Default for InterfaceRegistry {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isolve_requires_only_solve_arity_1() {
        let reg = InterfaceRegistry::new();
        let members = reg.get("ISolve").expect("ISolve must be seeded");
        assert_eq!(members, &[InterfaceFn { name: "solve".to_string(), arity: 1 }]);
    }

    #[test]
    fn isolver_requires_parse_solve_verify_in_order() {
        let reg = InterfaceRegistry::new();
        let members = reg.get("ISolver").expect("ISolver must be seeded");
        let names: Vec<&str> = members.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, vec!["parse", "solve", "verify"]);
        assert_eq!(members.iter().find(|m| m.name == "parse").unwrap().arity, 1);
        assert_eq!(members.iter().find(|m| m.name == "solve").unwrap().arity, 1);
        assert_eq!(members.iter().find(|m| m.name == "verify").unwrap().arity, 2);
    }

    #[test]
    fn unknown_interface_name_resolves_to_none() {
        let reg = InterfaceRegistry::new();
        assert!(reg.get("INotReal").is_none());
    }

    #[test]
    fn isolverlearn_requires_parse_solve_verify_learn_in_order() {
        let reg = InterfaceRegistry::new();
        let members = reg.get("ISolverLearn").expect("ISolverLearn must be seeded");
        let names: Vec<&str> = members.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, vec!["parse", "solve", "verify", "learn"]);
        assert_eq!(members.iter().find(|m| m.name == "parse").unwrap().arity, 1);
        assert_eq!(members.iter().find(|m| m.name == "solve").unwrap().arity, 1);
        assert_eq!(members.iter().find(|m| m.name == "verify").unwrap().arity, 2);
        // Unlike the old inline `optional ... learn(...)` shape both gsm8k.cube
        // and conversation_agent.cube used to carry, the registry has no
        // optional-member concept -- learn is REQUIRED here, arity 3.
        assert_eq!(members.iter().find(|m| m.name == "learn").unwrap().arity, 3);
    }
}
