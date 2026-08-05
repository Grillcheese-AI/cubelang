//! VM-internal module/capability registry — the `use` system's backing
//! store (Task 3 of the CubeLang foundation cycle).
//!
//! A `.cube` program brings a module into scope with a top-level
//! `use <name>;` declaration (`ast::TopLevel::Use`, parsed by
//! `parser::Parser::parse_use`). That makes the module's function names
//! resolvable as bare calls inside the whole compilation unit — DENY BY
//! DEFAULT: a module the registry knows about but a program did NOT `use`
//! is invisible to call resolution (`ModuleRegistry::resolve`), even though
//! `ModuleRegistry::get` would still find it directly (used for compile-time
//! `override` validation and for seeding the VM's name table, neither of
//! which is itself a grant of call access).
//!
//! A program may reclaim a used module's name for itself by declaring
//! `function name() override { ... }` — a POSTFIX marker on the function
//! signature (`ast::FunctionSig::is_override`), distinct from the
//! pre-existing PREFIX `override` modifier SPEC.md reserves for a different,
//! not-yet-built feature (inheritance-style overriding inside an `extend`
//! block; this task does not touch it). An `override` is only valid when the
//! name is both present in a `use`'d module AND marked `overridable` there;
//! `compiler::Compiler::check_override_validity` enforces this at compile
//! time, so by the time bytecode exists an `is_override == true` function is
//! always a legitimate one — see `ast::FunctionSig::is_override`'s doc.
//!
//! Call resolution precedence, implemented in `vm::engine`'s `op::CALL`:
//! program's validated override > registry implementation > plain
//! program-local function (today's pre-Task-3 behaviour, unchanged) > error.
//!
//! `__wiring__`: WIRED. `VM::new` seeds `self.registry` and pre-registers
//! every registry function name into the hash-reversal table; `op::CALL`
//! consults the registry on every call.
//!
//! This module seeds a `demo` module (Task 3's own test fixture: an
//! overridable `greet` and a sealed, non-overridable `sealed`) and, as of
//! Task 6, a real `vsa` module — `recover(reg, role)`, which performs
//! genuine UNBIND+cleanup (`VM::vsa_unbind_cleanup_known`) and returns the
//! recovered `Value::Str` symbol, so a generated CubeLang program never
//! needs an `asm` block itself to do real VSA recovery. (Task 4's standard
//! interfaces, `ISolve`/`ISolver`, went into a separate sibling registry —
//! `vm::interfaces::InterfaceRegistry` — not here; nothing about THIS
//! registry's shape is demo-specific, or vsa-specific for that matter.)
//! There is deliberately no registry-native `bind`/`plant` counterpart:
//! `bind` is itself a reserved opcode keyword (`TokenKind::OpBind`) so a
//! same-named registry function would be unreachable as a call expression,
//! and the existing `bind`/`bind_role` statements already cover planting
//! with no functional gap for `recover` to fill — see the Task 6 report.

use std::collections::HashMap;

use super::engine::{Value, VM};

/// VM registry format version. The point of a NAME-ADDRESSED table (as
/// opposed to a positional one) is that `use vsa;` means the same thing
/// across runs of a given VM build; bump this if a future change alters
/// what an existing module name resolves to in a way an existing program
/// would notice. A constant is enough for now — nothing branches on it yet.
pub const REGISTRY_VERSION: u32 = 1;

/// A native (Rust) implementation of a registry function. Takes the running
/// VM — so a real module (e.g. Task 6's `vsa` helper) can reach the VSA
/// codebook, hippocampal memory, registers, ... — and the call's
/// already-resolved argument values. Returns the call's result the same way
/// an intra-program CALL does (the caller pushes it onto `VM::stack`).
pub type NativeFn = fn(&mut VM, &[Value]) -> Value;

/// One function a module exposes.
#[derive(Clone, Copy)]
pub struct ModuleFn {
    /// Whether a program may shadow this with its own `function name()
    /// override { ... }`. `false` seals the capability: no program-supplied
    /// body can ever run in its place — declaring `override` on it is a
    /// compile error, not a way in.
    pub overridable: bool,
    pub call: NativeFn,
}

/// One module's exposed surface: fn name → its registry entry.
struct Module {
    functions: HashMap<String, ModuleFn>,
}

/// The VM-internal, name-addressed module registry: module name → its
/// functions. See the module doc for the deny-by-default access model.
pub struct ModuleRegistry {
    pub version: u32,
    modules: HashMap<String, Module>,
}

impl ModuleRegistry {
    /// Build the registry seeded with every module the VM ships. Cheap and
    /// deterministic — callers each build their own copy (the compiler, for
    /// `override` validation; the VM, for call resolution) rather than
    /// sharing one instance. There is no dynamic registration in this task;
    /// a new module is added by extending this constructor.
    pub fn new() -> Self {
        let mut modules = HashMap::new();
        modules.insert("demo".to_string(), demo_module());
        modules.insert("vsa".to_string(), vsa_module());
        Self { version: REGISTRY_VERSION, modules }
    }

    /// Look up `name` inside `module`, regardless of whether any program
    /// has `use`'d it. This is the right question for compile-time
    /// `override` validation ("does this capability exist and is it
    /// overridable") and for seeding the VM's hash-reversal table with
    /// every registry name up front (see `VM::new`) — neither of which
    /// should be gated by a particular program's `use` list.
    pub fn get(&self, module: &str, name: &str) -> Option<&ModuleFn> {
        self.modules.get(module)?.functions.get(name)
    }

    /// Resolve `name` against `used` (a program's `used_modules`, in
    /// declaration order), returning the first hit. DENY BY DEFAULT: a
    /// module absent from `used` is invisible here even if the registry
    /// otherwise knows it — `get` is the only way to reach it un-gated.
    ///
    /// First hit, not best hit: if more than one `use`'d module happened to
    /// expose the same `name` (impossible with today's single `demo`
    /// module, but not once a second module exists), this — like
    /// `op::CALL` — stops at whichever comes first in `used`'s order and
    /// never looks at the rest, regardless of what they'd say.
    pub fn resolve<'a>(&'a self, used: &[String], name: &str) -> Option<&'a ModuleFn> {
        used.iter().find_map(|m| self.get(m, name))
    }

    /// Same lookup as `resolve`, but also names the module that won.
    /// `resolve` stays reference-returning and allocation-free for
    /// `op::CALL`'s hot path; this clones the module name for callers that
    /// need it for a diagnostic (e.g. `Compiler::check_sealed_collision`)
    /// and can afford to. Must agree with `resolve` on WHICH module wins —
    /// same `used`-order, first-hit-not-best-hit semantics — or a
    /// diagnostic could name a different module than the one that will
    /// actually be reached at runtime.
    pub fn resolve_named(&self, used: &[String], name: &str) -> Option<(String, ModuleFn)> {
        used.iter().find_map(|m| self.get(m, name).map(|f| (m.clone(), *f)))
    }

    /// Every function name exposed by any module, across the whole
    /// registry — regardless of `use`. The VM pre-registers these into its
    /// hash-reversal table at startup (`VM::new`) so `op::CALL` can resolve
    /// the name STRING at all; that bookkeeping is not a grant of access
    /// (deny-by-default is `resolve`'s job, gated on a program's own
    /// `used_modules`), it only makes a hash the compiler already emitted
    /// reversible.
    pub fn all_function_names(&self) -> impl Iterator<Item = &str> {
        self.modules.values().flat_map(|m| m.functions.keys().map(String::as_str))
    }
}

impl Default for ModuleRegistry {
    fn default() -> Self { Self::new() }
}

/// Task 3's own test fixture. `greet` is overridable (a program may replace
/// it); `sealed` is not (declaring `override` on it is a compile error).
/// Both ignore their arguments and the VM handle — real modules (Tasks 4/6)
/// are expected to use both.
fn demo_module() -> Module {
    let mut functions = HashMap::new();
    functions.insert("greet".to_string(), ModuleFn {
        overridable: true,
        call: |_vm, _args| Value::Int(42),
    });
    functions.insert("sealed".to_string(), ModuleFn {
        overridable: false,
        call: |_vm, _args| Value::Int(7),
    });
    Module { functions }
}

/// Task 6: the `vsa` registry module — hides the VM's real VSA bind/unbind
/// (BIND_ROLE/UNBIND's own primitives, already real: `VM::vsa_bind_into` /
/// `VM::vsa_unbind_cleanup`) behind one ordinary-looking function call,
/// `recover(reg, role)`, so a generated CubeLang program can do genuine
/// role-filler recovery without ever writing an `asm` block itself — `asm`
/// is for BUILDING helpers like this one; `recover` is what a generated
/// program actually calls (this is what the separate reasoning-bridge slice
/// writes against, per the design doc). `recover` is overridable, like
/// `demo::greet` — there is no reason to seal a general-purpose helper the
/// way `demo::sealed` intentionally is for Task 3's own test.
fn vsa_module() -> Module {
    let mut functions = HashMap::new();
    functions.insert("recover".to_string(), ModuleFn {
        overridable: true,
        call: vsa_recover,
    });
    Module { functions }
}

/// `recover(reg, role)` — UNBIND + cosine cleanup against every symbol the
/// VM currently knows (`VM::vsa_unbind_cleanup_known`), returned as
/// `Value::Str`. Both arguments arrive by their NAME identity (a register
/// key / a role symbol string) via `VM::resolve_registry_arg`, not by
/// resolving to a register's current value — see that method's doc for why.
/// `Value::Null` when `reg` never held a bound hypervector (nothing was
/// ever planted there) or either argument didn't resolve to a string at all
/// (a malformed call — e.g. an immediate where a name was expected).
fn vsa_recover(vm: &mut VM, args: &[Value]) -> Value {
    let (Some(Value::Str(reg)), Some(Value::Str(role))) = (args.get(0), args.get(1)) else {
        return Value::Null;
    };
    match vm.vsa_unbind_cleanup_known(reg, role) {
        Some((symbol, _similarity)) => Value::Str(symbol),
        None => Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_module_is_seeded_and_callable() {
        let reg = ModuleRegistry::new();
        let greet = reg.get("demo", "greet").expect("demo.greet must exist");
        assert!(greet.overridable);
        let mut vm = VM::new();
        assert_eq!((greet.call)(&mut vm, &[]).as_i64(), 42);
    }

    #[test]
    fn sealed_fn_exists_but_is_not_overridable() {
        let reg = ModuleRegistry::new();
        let sealed = reg.get("demo", "sealed").expect("demo.sealed must exist");
        assert!(!sealed.overridable);
        let mut vm = VM::new();
        assert_eq!((sealed.call)(&mut vm, &[]).as_i64(), 7);
    }

    #[test]
    fn get_finds_a_module_regardless_of_use() {
        // get() is the un-gated lookup -- used for override validation and
        // name-table seeding, neither of which should require `use`.
        let reg = ModuleRegistry::new();
        assert!(reg.get("demo", "greet").is_some());
        assert!(reg.get("demo", "nonexistent").is_none());
        assert!(reg.get("nonexistent_module", "greet").is_none());
    }

    #[test]
    fn resolve_is_deny_by_default() {
        let reg = ModuleRegistry::new();
        // "demo" exists in the registry, but resolve only ever looks inside
        // `used` -- absent from it, "demo" must not resolve.
        assert!(reg.resolve(&[], "greet").is_none());
        assert!(reg.resolve(&["other".to_string()], "greet").is_none());
        let hit = reg.resolve(&["demo".to_string()], "greet");
        assert!(hit.is_some_and(|f| f.overridable));
    }

    #[test]
    fn resolve_named_agrees_with_resolve() {
        let reg = ModuleRegistry::new();
        assert!(reg.resolve_named(&[], "greet").is_none());
        let (module, entry) = reg.resolve_named(&["demo".to_string()], "sealed")
            .expect("demo.sealed exists");
        assert_eq!(module, "demo");
        assert!(!entry.overridable);
    }

    #[test]
    fn all_function_names_covers_every_seeded_module_fn() {
        let reg = ModuleRegistry::new();
        let names: Vec<&str> = reg.all_function_names().collect();
        assert!(names.contains(&"greet"));
        assert!(names.contains(&"sealed"));
        assert!(names.contains(&"recover"));
    }

    // ── Task 6: `vsa` module ────────────────────────────────────────────

    #[test]
    fn vsa_module_is_seeded_and_recover_is_overridable() {
        let reg = ModuleRegistry::new();
        let recover = reg.get("vsa", "recover").expect("vsa.recover must exist");
        assert!(recover.overridable, "recover should be a general-purpose, overridable helper");
    }

    #[test]
    fn recover_is_deny_by_default_like_every_other_module() {
        let reg = ModuleRegistry::new();
        assert!(reg.resolve(&[], "recover").is_none());
        assert!(reg.resolve(&["demo".to_string()], "recover").is_none());
        assert!(reg.resolve(&["vsa".to_string()], "recover").is_some());
    }

    #[test]
    fn recover_on_an_unbound_register_key_returns_null() {
        // Unit-level check of `vsa_recover` in isolation (no compiler/CALL
        // involved): a register key that was never bound to a hypervector
        // recovers no symbol.
        let reg = ModuleRegistry::new();
        let recover = reg.get("vsa", "recover").expect("vsa.recover must exist");
        let mut vm = VM::new();
        let args = [Value::Str("r_never_bound".to_string()), Value::Str("SUBJECT".to_string())];
        assert!(matches!((recover.call)(&mut vm, &args), Value::Null));
    }

    #[test]
    fn recover_malformed_args_return_null_rather_than_panicking() {
        let reg = ModuleRegistry::new();
        let recover = reg.get("vsa", "recover").expect("vsa.recover must exist");
        let mut vm = VM::new();
        assert!(matches!((recover.call)(&mut vm, &[]), Value::Null), "zero args");
        assert!(matches!((recover.call)(&mut vm, &[Value::Int(1), Value::Int(2)]), Value::Null),
            "non-Str args");
    }

    // A full plant-then-recover round trip through REAL compiled source
    // (`use vsa; ... bind frame, SUBJECT, "cat"; ... recover(frame,
    // SUBJECT)`) is covered end-to-end by `tests/asm_vsa.rs` -- that path
    // exercises the compiler's symbol recording too (`record_symbol` for
    // the role, `emit_global` for the filler), which a registry-level unit
    // test here would otherwise have to fake by hand.
}
