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
//! This module seeds only a `demo` module, Task 3's own test fixture (an
//! overridable `greet` and a sealed, non-overridable `sealed`). Tasks 4
//! (standard interfaces) and 6 (the `vsa` helper) are expected to add real
//! modules here, alongside `demo` — nothing about the registry shape is
//! demo-specific.

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
    pub fn resolve<'a>(&'a self, used: &[String], name: &str) -> Option<&'a ModuleFn> {
        used.iter().find_map(|m| self.get(m, name))
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
    fn all_function_names_covers_every_seeded_module_fn() {
        let reg = ModuleRegistry::new();
        let names: Vec<&str> = reg.all_function_names().collect();
        assert!(names.contains(&"greet"));
        assert!(names.contains(&"sealed"));
    }
}
