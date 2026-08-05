//! Task 5: the `import` loader pre-pass — external user-file modularity.
//!
//! `import "path.cube";` is a NEW top-level declaration (`TopLevel::Import`,
//! see `ast.rs`), distinct from Task 3's `use <name>;` (a VM-INTERNAL
//! registry module, name-addressed, no filesystem involved) and distinct
//! from the old in-body ES-module `Stmt::Import`/`ImportStmt` deleted in
//! Task 1 for being the wrong shape (a statement can't introduce new
//! top-level names).
//!
//! This module resolves `TopLevel::Import` declarations: it reads the entry
//! file, recursively follows every `import` it finds (resolved relative to
//! the FILE THAT DECLARES IT, not necessarily the entry file), and
//! AST-merges every reachable file's top-level declarations into one
//! `SourceFile`. The merge happens at the AST level, not by concatenating
//! source text: each declaration keeps the `Span` its OWN file's
//! lexer/parser gave it (line/col relative to that file), so `--strict`
//! diagnostics deep inside imported code still cite a real source
//! location — see `LoadError::Parse`, which additionally names the file,
//! since `token::Span` itself has no file field (a pre-existing limitation;
//! see `LoadError::Parse`'s doc for how this module works around it for its
//! OWN errors, and the Task 5 report for the residual gap deeper in the
//! compiler's own error paths).
//!
//! The resulting `SourceFile` feeds `compiler::compile_ast` /
//! `compile_ast_strict` — the `SourceFile`-input siblings of the existing
//! `&str`-input `compile`/`compile_strict`, which stay untouched (this
//! module never threads a base directory through them; it fully resolves
//! and merges BEFORE compilation ever runs, per the Task 5 brief). Imported
//! code is verified exactly like the entry file's own: nothing here bypasses
//! `--strict`.
//!
//! ## Ordering
//!
//! A file's own declarations always precede what it imports, and nested
//! imports expand depth-first in the order their `import` statements
//! appear in source. Concretely: `expand(file) = own_items(file) ++
//! expand(import_1) ++ expand(import_2) ++ ...`. This keeps `main.rs`'s
//! "the first compiled program is the default one to run" behavior
//! (`cmd_run`) pointed at the ENTRY file's own program even when it
//! `import`s a file that happens to declare a `program` of its own — an
//! import ordered first in the merged text would otherwise silently
//! become the default-run program, which is almost never what "I imported
//! a helper library" should mean.
//!
//! ## Cycle / diamond handling
//!
//! A canonicalized (`Path::canonicalize`) visited-set is threaded through
//! the whole recursive load. A file already in the set — whether because
//! it's a genuine cycle (`a` imports `b` imports `a`) or an ordinary
//! diamond (two different files both import the same third file) — is
//! treated as already-loaded and silently skipped: a no-op, not an error.
//! The design distinguishes "cycle detection" (this) from
//! "error-on-duplicate-name" (below), reserving the hard error for the
//! latter — a re-visited file is not itself a defect. This also happens to
//! be exactly the right behavior for diamonds (load the shared file's
//! declarations once, not once per importer) for free, via the same
//! mechanism, with no special-casing needed.
//!
//! ## Duplicate names
//!
//! Two files — the entry included — declaring the same top-level NAME
//! (interface, program, container, struct, enum, type alias, or event) is a
//! hard error, by design — it forces real de-duplication rather than a
//! silent last-writer-wins merge, so it is a feature, not a limitation to
//! work around. `use` (a reference to a VM registry module, not a declaration of
//! a new name) and `extend` (extends an EXISTING target, doesn't introduce
//! one) are exempt — see `declared_name`.
//!
//! ## What gets exposed on import — judgment call, documented here
//!
//! The intent is for imported files to expose reusable declarations —
//! interfaces, structs, enums, type aliases, and the helper functions or
//! programs the user wants to reuse. The exact vehicle is a judgment call,
//! resolved here by what the grammar actually allows. The
//! current grammar has no free-standing top-level function: the only place
//! a `fn` lives outside a `program`/`container`/`extend` body is inside one
//! of those. So this loader merges every non-`Import`, non-`Use`-duplicate
//! top-level item verbatim — including whole `program` blocks — since
//! that's the only vehicle the language has today for a reusable function.
//! What this does NOT do: make a merged-in `program`'s functions callable
//! by bare or qualified name from a DIFFERENT program's function body.
//! Cross-program calls aren't wired anywhere in this compiler yet (checked:
//! `compiler.rs`'s `MethodCall` lowering discards its receiver expression
//! entirely and treats the method name as a plain global function name;
//! `vm/engine.rs`'s `op::CALL` resolution precedence is own-program-override
//! → used-module-registry → own-program-plain-function → error, with no
//! cross-program lookup at any step) — that would be a real, separate
//! language feature, not a loader concern. "Reusing a helper fn" from an
//! import today concretely means: the imported `program` is merged in as a
//! complete, independently-loadable/callable unit (`vm.call(imported_name,
//! fn_name, args)`), or its functionality is reused through the already-
//! real `implements`-against-an-in-file-`interface` mechanism (Task 4),
//! which the loader makes reachable across files for free simply by
//! merging the `interface` decl in before `Compiler::compile` gathers
//! `inline_interfaces`. See `tests/file_import.rs` case (a) and the task
//! report for the worked example.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::ast::{SourceFile, TopLevel};
use crate::parser::ParseError;

/// Everything that can go wrong resolving `import`s, with enough context
/// (a real file path, not just a bare `Span`) to point at the actual
/// problem across a multi-file compilation.
#[derive(Debug)]
pub enum LoadError {
    /// Couldn't read a file — the entry, or a resolved `import` target
    /// (nonexistent path, permissions, etc.). `Path::canonicalize` failing
    /// (the target doesn't exist) also surfaces here.
    Io { path: PathBuf, error: std::io::Error },
    /// A file was read fine but failed to parse. Carries the file's path
    /// alongside the underlying `ParseError` (which only has a bare
    /// line/col — see the module doc) so the message stays unambiguous
    /// once more than one file is in play.
    Parse { path: PathBuf, error: ParseError },
    /// Two files — the entry included — declared the same top-level name.
    /// `first`/`second` are the two files, in load order (not necessarily
    /// textual/alphabetical order) — whichever was reached first "owns"
    /// the name.
    DuplicateName { name: String, first: PathBuf, second: PathBuf },
}

/// `Path::canonicalize` on Windows returns a `\\?\`-prefixed "verbatim"
/// path (e.g. `\\?\C:\Users\...`) — correct as a HashMap/HashSet key
/// (exactly why `load_into` canonicalizes before comparing), but ugly and
/// confusing in a message a person reads. This affects ONLY display: the
/// `PathBuf`s stored in `LoadError`/`visited`/`names` stay fully
/// canonicalized, so correctness is untouched. A no-op on non-Windows
/// paths, which never have this prefix.
fn display_path(p: &Path) -> String {
    let s = p.display().to_string();
    s.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(s)
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Io { path, error } =>
                write!(f, "cannot read {}: {}", display_path(path), error),
            LoadError::Parse { path, error } =>
                write!(f, "{}: {}", display_path(path), error),
            LoadError::DuplicateName { name, first, second } => write!(
                f,
                "duplicate top-level name `{}`: declared in both {} and {}",
                name, display_path(first), display_path(second),
            ),
        }
    }
}

impl std::error::Error for LoadError {}

/// Load `entry` and everything it transitively `import`s into one merged
/// `SourceFile`. See the module doc for ordering / cycle / duplicate-name
/// rules. This is the ONLY public entry point `main.rs` needs — it wraps
/// the read-file + `parser::parse` step for every file it touches, so
/// callers no longer call `std::fs::read_to_string`/`parser::parse`
/// directly for a `.cube` entry point.
pub fn load(entry: &Path) -> Result<SourceFile, LoadError> {
    let mut visited: HashSet<PathBuf> = HashSet::new();
    let mut names: HashMap<String, PathBuf> = HashMap::new();
    let mut merged: Vec<TopLevel> = Vec::new();
    load_into(entry, &mut visited, &mut names, &mut merged)?;
    Ok(SourceFile { items: merged })
}

/// The top-level name a `TopLevel` item introduces, for duplicate-name
/// checking — `None` for items that don't declare a new name at all
/// (`Use`: references an existing VM module; `ExtendBlock`: extends an
/// existing target; `Import`: never reaches here, see `load_into`).
fn declared_name(item: &TopLevel) -> Option<&str> {
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

/// Recursively load `path`, merging its own declarations (duplicate-name
/// checked against everything merged so far) followed by whatever it
/// imports, depth-first, into `merged`. `visited`/`names` accumulate across
/// the whole call tree so cycles/diamonds and cross-file collisions are
/// caught regardless of how deep they are.
fn load_into(
    path: &Path,
    visited: &mut HashSet<PathBuf>,
    names: &mut HashMap<String, PathBuf>,
    merged: &mut Vec<TopLevel>,
) -> Result<(), LoadError> {
    // Canonicalize FIRST: this is both the cycle/diamond dedup key (so
    // `./lib.cube`, `lib.cube`, and `../x/lib.cube` from different
    // importers all recognize each other as the same file) and the
    // existence check (a nonexistent path fails here with a real io::Error,
    // e.g. NotFound, rather than a confusing downstream failure).
    let canonical = path.canonicalize()
        .map_err(|error| LoadError::Io { path: path.to_path_buf(), error })?;

    if !visited.insert(canonical.clone()) {
        // Already loaded (or an ancestor of `path` in the current recursion
        // — i.e. a cycle). Either way: a no-op, not an error, per the brief.
        return Ok(());
    }

    let source = std::fs::read_to_string(&canonical)
        .map_err(|error| LoadError::Io { path: canonical.clone(), error })?;
    let ast = crate::parser::parse(&source)
        .map_err(|error| LoadError::Parse { path: canonical.clone(), error })?;

    // Split this file's items into "its own" vs. "what it imports" —
    // own items merge first (see module doc "Ordering"), imports are
    // resolved and recursed into afterward, in the order they appeared.
    let mut own_items = Vec::with_capacity(ast.items.len());
    let mut import_paths = Vec::new();
    for item in ast.items {
        match item {
            TopLevel::Import(p) => import_paths.push(p),
            other => own_items.push(other),
        }
    }

    for item in own_items {
        if let Some(name) = declared_name(&item) {
            if let Some(first) = names.get(name) {
                return Err(LoadError::DuplicateName {
                    name: name.to_string(),
                    first: first.clone(),
                    second: canonical,
                });
            }
            names.insert(name.to_string(), canonical.clone());
        }
        merged.push(item);
    }

    // Resolve each import relative to THIS file's own directory (not the
    // original entry's) -- `Path::join` already does the right thing if
    // `import_path` happens to be absolute (it discards `base` entirely),
    // so no special-casing is needed for that case.
    let base = canonical.parent().map(Path::to_path_buf).unwrap_or_default();
    for import_path in import_paths {
        let resolved = base.join(&import_path);
        load_into(&resolved, visited, names, merged)?;
    }

    Ok(())
}
