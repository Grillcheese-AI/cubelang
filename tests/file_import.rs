//! Task 5: `import "path.cube";` — external user-file modularity.
//!
//! A NEW top-level declaration (`TopLevel::Import`) + a loader pre-pass
//! (`src/loader.rs`) that resolves it: reads the entry file, recursively
//! follows every `import` (relative to whichever file declares it), and
//! AST-merges every reachable file's top-level declarations into one
//! `SourceFile` before compilation ever runs. Distinct from Task 3's `use`
//! (a VM-internal registry module reference, no filesystem involved) and
//! from the old in-body ES-module `Stmt::Import` deleted in Task 1.
//!
//! These are the brief's required cases (a)-(d); the rest (diamond dedup,
//! ordering, --strict-still-applies-to-imported-code) pin down supporting
//! behaviour discovered while building the loader — see the task report
//! for the reasoning behind each, especially case (a)'s "what does
//! `import` actually expose" judgment call.

use std::path::Path;

use cubelang::ast::TopLevel;
use cubelang::compiler;
use cubelang::loader;
use cubelang::vm::{ExecResult, VM};

fn fixture(name: &str) -> std::path::PathBuf {
    Path::new("tests/fixtures").join(name)
}

/// Top-level declared name, mirroring `loader::declared_name` (private to
/// the loader) — kept separate on purpose: this is the test asserting on
/// the AST shape it cares about, not reaching into the loader's internals.
fn top_level_name(item: &TopLevel) -> Option<&str> {
    match item {
        TopLevel::Interface(d) => Some(&d.name),
        TopLevel::Program(d) => Some(&d.name),
        TopLevel::Container(d) => Some(&d.name),
        TopLevel::Struct(d) => Some(&d.name),
        TopLevel::Enum(d) => Some(&d.name),
        TopLevel::TypeAlias(d) => Some(&d.name),
        TopLevel::EventDecl(d) => Some(&d.name),
        TopLevel::ExtendBlock(_) | TopLevel::Use(_) | TopLevel::Import(_) => None,
    }
}

// ── (a) lib.cube's helper fn + interface, imported and used, compiles+runs ─

#[test]
fn a_import_merges_interface_and_program_then_compiles_and_runs() {
    let merged = loader::load(&fixture("uses_lib.cube")).expect("load + merge uses_lib.cube");

    // Both lib.cube's declarations made it into the merge.
    let names: Vec<&str> = merged.items.iter().filter_map(top_level_name).collect();
    assert!(names.contains(&"ILib"), "ILib missing from merge: {:?}", names);
    assert!(names.contains(&"Lib"), "Lib missing from merge: {:?}", names);
    assert!(names.contains(&"UsesLib"), "UsesLib missing from merge: {:?}", names);

    let programs = compiler::compile_ast(&merged).expect("merged AST must compile");
    assert_eq!(programs.len(), 2, "expected Lib + UsesLib: {:?}",
        programs.iter().map(|p| &p.name).collect::<Vec<_>>());

    // UsesLib really did resolve `implements ILib` against the INTERFACE
    // THAT ONLY EXISTS BECAUSE lib.cube WAS MERGED IN -- this is the
    // load-bearing proof the merge is real, not just cosmetic: without
    // ILib present, `implements ILib` would fail to resolve at all (Task 4's
    // check_implements errors when a name is neither a registry interface
    // nor an in-file inline decl).
    let uses_lib = programs.iter().find(|p| p.name == "UsesLib").expect("UsesLib compiled");
    assert_eq!(uses_lib.implements, vec!["ILib".to_string()]);

    // Both programs actually run: Lib's helper() (the imported "helper fn")
    // and UsesLib's own helper() (satisfying the imported interface).
    let mut vm = VM::new();
    for p in programs {
        vm.load(p);
    }
    match vm.call("Lib", "helper", vec![]) {
        ExecResult::Return(v) | ExecResult::Ok(v) => assert_eq!(v.as_i64(), 42),
        other => panic!("Lib::helper() should return 42, got {:?}", other),
    }
    match vm.call("UsesLib", "helper", vec![]) {
        ExecResult::Return(v) | ExecResult::Ok(v) => assert_eq!(v.as_i64(), 42),
        other => panic!("UsesLib::helper() should return 42, got {:?}", other),
    }
}

#[test]
fn a_imported_code_is_still_verified_under_strict_not_bypassed() {
    // "Imported code is verified like any code... import is modularity,
    // never a bypass" (Task 5 brief). strict_violation_lib.cube's `unify`
    // opcode is trace-only; it must surface as a --strict error even though
    // it only reaches the compile via strict_violation_entry.cube's import.
    let merged = loader::load(&fixture("strict_violation_entry.cube")).expect("load + merge");
    let err = compiler::compile_ast_strict(&merged)
        .expect_err("unify is trace-only -- must be rejected under --strict, import or not");
    assert!(err.contains("unify"), "{}", err);
    // The offending PROGRAM is named in the error even though Span itself
    // carries no file -- current_program::current_function still
    // disambiguates which file's code tripped the check.
    assert!(err.contains("StrictViolationLib"), "{}", err);
}

// ── (b) two imported files declaring the same top-level name -> error ─────

#[test]
fn b_duplicate_top_level_name_across_two_imported_files_is_an_error() {
    let err = loader::load(&fixture("dup_entry.cube"))
        .expect_err("dup_a.cube and dup_b.cube both declare `struct Shared`");
    let msg = err.to_string();
    assert!(msg.contains("Shared"), "{}", msg);
    assert!(msg.contains("dup_a.cube"), "{}", msg);
    assert!(msg.contains("dup_b.cube"), "{}", msg);
}

#[test]
fn b_a_file_colliding_with_the_entrys_own_name_is_also_an_error() {
    // The brief is explicit: "two files (OR A FILE AND THE ENTRY)".
    // dup_with_entry.cube declares its own `struct Shared` AND imports
    // dup_a.cube, which also declares `struct Shared` -- this pins down
    // the entry-vs-import collision independently of the
    // two-different-imports case above.
    let err = loader::load(&fixture("dup_with_entry.cube"))
        .expect_err("entry's own `Shared` collides with dup_a.cube's");
    let msg = err.to_string();
    assert!(msg.contains("Shared"), "{}", msg);
}

// ── (c) an import cycle resolves cleanly, no hang, no duplication ─────────

#[test]
fn c_an_import_cycle_resolves_cleanly_without_hanging_or_duplicating() {
    // The brief's Build section is explicit about the mechanism: "thread a
    // canonicalized visited-set; on revisit, treat as already-loaded
    // (no-op), don't infinite-loop" -- a cycle is a no-op, not an error.
    // Simply returning at all (this test has a finite runtime under the
    // normal `cargo test` timeout) is itself part of what's being checked:
    // an infinite-recursion bug here would hang or stack-overflow, not
    // fail an assertion.
    let merged = loader::load(&fixture("cycle_a.cube"))
        .expect("a<->b cycle must resolve cleanly, not error");

    let names: Vec<&str> = merged.items.iter().filter_map(top_level_name).collect();
    assert_eq!(names.iter().filter(|&&n| n == "FromA").count(), 1,
        "FromA must appear exactly once: {:?}", names);
    assert_eq!(names.iter().filter(|&&n| n == "FromB").count(), 1,
        "FromB must appear exactly once: {:?}", names);
}

#[test]
fn c_entering_the_cycle_from_the_other_file_also_resolves_cleanly() {
    // Symmetry check: b -> a -> b must behave the same as a -> b -> a.
    let merged = loader::load(&fixture("cycle_b.cube")).expect("b<->a cycle must resolve cleanly");
    let names: Vec<&str> = merged.items.iter().filter_map(top_level_name).collect();
    assert_eq!(names.iter().filter(|&&n| n == "FromA").count(), 1);
    assert_eq!(names.iter().filter(|&&n| n == "FromB").count(), 1);
}

// ── (d) a path relative to the IMPORTING file (not the entry) resolves ────

#[test]
fn d_import_path_resolves_relative_to_the_importing_file_not_the_entry() {
    // relpath/entry.cube imports "sub/mid.cube"; sub/mid.cube imports the
    // BARE "leaf.cube", which only exists at relpath/sub/leaf.cube -- there
    // is no relpath/leaf.cube. If resolution were (incorrectly) always
    // relative to the entry's directory, this load would fail with an Io
    // error (file not found). A clean load proves resolution is relative
    // to whichever file DECLARES the import.
    let merged = loader::load(&fixture("relpath/entry.cube"))
        .expect("leaf.cube must resolve relative to sub/mid.cube's own directory");

    let names: Vec<&str> = merged.items.iter().filter_map(top_level_name).collect();
    assert!(names.contains(&"RelEntry"), "{:?}", names);
    assert!(names.contains(&"FromMid"), "{:?}", names);
    assert!(names.contains(&"FromLeaf"), "sub/leaf.cube was not reached: {:?}", names);
}

// ── Supporting: ordering — a file's own decls precede what it imports ─────

#[test]
fn entry_files_own_declarations_precede_its_imports_in_merge_order() {
    // Matters for main.rs's cmd_run, which runs `programs[0]` by default --
    // an import ordered first in merged.items must not silently become the
    // default-run program just because the `import` statement happened to
    // be the first line of the entry file.
    let merged = loader::load(&fixture("uses_lib.cube")).expect("load + merge");
    let names: Vec<&str> = merged.items.iter().filter_map(top_level_name).collect();
    let uses_lib_pos = names.iter().position(|&n| n == "UsesLib").expect("UsesLib present");
    let lib_pos = names.iter().position(|&n| n == "Lib").expect("Lib present");
    assert!(uses_lib_pos < lib_pos,
        "entry's own UsesLib ({}) must precede imported Lib ({}): {:?}",
        uses_lib_pos, lib_pos, names);
}

// ── Supporting: a diamond import loads the shared file exactly once ───────

#[test]
fn a_diamond_import_is_only_merged_once() {
    // diamond_entry.cube imports both diamond_x.cube and diamond_y.cube,
    // which BOTH import diamond_common.cube. Not a cycle (no path leads
    // back to diamond_entry.cube), but the same visited-set mechanism must
    // dedup it: diamond_common's declarations should appear exactly once,
    // not twice, and this must NOT be a duplicate-name error either (it's
    // the same file reached twice, not two different files colliding).
    let merged = loader::load(&fixture("diamond_entry.cube"))
        .expect("a diamond (not a cycle) must load cleanly, not error");
    let names: Vec<&str> = merged.items.iter().filter_map(top_level_name).collect();
    assert_eq!(names.iter().filter(|&&n| n == "FromCommon").count(), 1,
        "diamond_common.cube must be merged exactly once: {:?}", names);
    assert!(names.contains(&"FromX"));
    assert!(names.contains(&"FromY"));
}

// ── Supporting: loading a nonexistent import target fails loudly ──────────

#[test]
fn a_nonexistent_import_target_is_a_clean_io_error_not_a_panic() {
    let tmp = std::env::temp_dir().join("cubelang_import_test_missing_target.cube");
    std::fs::write(&tmp, "import \"does_not_exist_anywhere.cube\";\n").expect("write temp entry");
    let err = loader::load(&tmp).expect_err("the imported file does not exist");
    assert!(matches!(err, loader::LoadError::Io { .. }), "{:?}", err);
    let _ = std::fs::remove_file(&tmp);
}

// ── Supporting: a file with no imports at all loads identically to a
// direct parser::parse (backward-compat guard for main.rs's rewiring) ─────

#[test]
fn a_file_with_no_imports_merges_to_exactly_its_own_items() {
    let direct = cubelang::parser::parse(
        &std::fs::read_to_string(fixture("diamond_common.cube")).unwrap()
    ).expect("direct parse");
    let via_loader = loader::load(&fixture("diamond_common.cube")).expect("loader, no imports");
    assert_eq!(direct.items.len(), via_loader.items.len());
    assert_eq!(
        direct.items.iter().filter_map(top_level_name).collect::<Vec<_>>(),
        via_loader.items.iter().filter_map(top_level_name).collect::<Vec<_>>(),
    );
}
