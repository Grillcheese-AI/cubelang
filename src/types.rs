//! Types (2026-10-24): what a declared type means, checked at compile time on what the compiler can
//! see (literals, struct literals, calls with literal arguments) and at run time on what crosses a
//! function's boundary (the host's arguments in, every return out).
//!
//! Until now a `TypeExpr` was decoration: carried through the AST, consulted by nothing, so
//! `solve(n: i64)` happily took `"seven"`. The rules here are deliberately the ones a skill can rely
//! on without breaking the programs already in use:
//!
//! - **`null` is a value of every type.** Absence is how a refusal is represented (`recover` of an
//!   absent role, an empty QUERY); a type check that refused it would turn every refusal into an error.
//! - **Lenient names check nothing:** `any`, `auto`, `number`, `quantity`, `symbol`, `void`, and the
//!   SPEC's AI-native types that have no runtime value yet (`emb`, `ctx`, `rag`, `module`, ...).
//!   `number` in particular is what existing programs write for a frame register that holds a
//!   hypervector. A name that is none of these, and no struct, enum or alias, is `Opaque`: checked by
//!   nothing at run time, and a compile error only under `--strict-types`.
//! - **Numbers widen, never narrow:** an int is a float; an integral float is an int (arithmetic
//!   collapses `4.0` to `4`); `3.5` is not an `i64`. Fixed-width ints carry their range.
//! - **Booleans are 0/1:** `true` compiles to `1`, so an int 0 or 1 is a `bool`.
//! - **Structs are structural:** a map with every field the struct declares (a defaulted field may be
//!   absent), each of its type, and no field it does not declare.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::ast::TypeExpr;
use crate::vm::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TypeSpec {
    Any,
    Int { min: i64, max: i64 },
    Float,
    Bool,
    Str,
    Hvec,
    Array(Box<TypeSpec>),
    Map(Box<TypeSpec>, Box<TypeSpec>),
    Tuple(Vec<TypeSpec>),
    Union(Vec<TypeSpec>),
    Nullable(Box<TypeSpec>),
    /// A struct, enum or alias of the source file, resolved through a `TypeTable`.
    Named(String),
    /// A name nothing declares (an interface's abstract `type Input;`, a typo). Checks nothing.
    Opaque(String),
}

/// The struct, enum and alias declarations a source file carries, by name.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TypeTable {
    pub structs: HashMap<String, StructDef>,
    pub enums: HashMap<String, Vec<String>>,
    pub aliases: HashMap<String, TypeSpec>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StructDef {
    pub name: String,
    /// (field, type, has a default) in declaration order.
    pub fields: Vec<(String, TypeSpec, bool)>,
}

/// Names that are checked by nothing: lenient on purpose (see the module doc).
pub const LENIENT: &[&str] = &[
    "any", "auto", "number", "quantity", "symbol", "void", "opcode", "role", "emb", "ctx", "rag",
    "module", "mdx", "agent", "cloned_agent", "file", "url", "dataset", "external_program", "request",
    "response", "function", "promise", "channel", "error", "Error", "byte",
];

fn int_range(name: &str) -> Option<(i64, i64)> {
    Some(match name {
        "i8" => (i8::MIN as i64, i8::MAX as i64),
        "i16" => (i16::MIN as i64, i16::MAX as i64),
        "i32" => (i32::MIN as i64, i32::MAX as i64),
        "i64" | "int" => (i64::MIN, i64::MAX),
        "u8" => (0, u8::MAX as i64),
        "u16" => (0, u16::MAX as i64),
        "u32" => (0, u32::MAX as i64),
        "u64" | "uint" => (0, i64::MAX),
        _ => return None,
    })
}

impl TypeSpec {
    /// The spec a source type expression denotes, given the file's declarations.
    pub fn from_expr(ty: &TypeExpr, decls: &TypeTable) -> TypeSpec {
        match ty {
            TypeExpr::Named(n) => Self::from_name(n, decls),
            TypeExpr::Array(t) | TypeExpr::Set(t) => TypeSpec::Array(Box::new(Self::from_expr(t, decls))),
            TypeExpr::Map(k, v) => TypeSpec::Map(Box::new(Self::from_expr(k, decls)), Box::new(Self::from_expr(v, decls))),
            TypeExpr::Tuple(ts) => TypeSpec::Tuple(ts.iter().map(|t| Self::from_expr(t, decls)).collect()),
            TypeExpr::Union(ts) => {
                // `T | null` is `T?`: the one union every example writes.
                let parts: Vec<&TypeExpr> = ts.iter().filter(|t| !matches!(t, TypeExpr::Named(n) if n == "null")).collect();
                let nullable = parts.len() < ts.len();
                let inner = if parts.len() == 1 {
                    Self::from_expr(parts[0], decls)
                } else {
                    TypeSpec::Union(parts.iter().map(|t| Self::from_expr(t, decls)).collect())
                };
                if nullable { TypeSpec::Nullable(Box::new(inner)) } else { inner }
            }
            TypeExpr::Nullable(t) => TypeSpec::Nullable(Box::new(Self::from_expr(t, decls))),
            TypeExpr::Promise(_) | TypeExpr::Channel(_) | TypeExpr::Fn(..) | TypeExpr::Void => TypeSpec::Any,
        }
    }

    pub fn from_name(n: &str, decls: &TypeTable) -> TypeSpec {
        if let Some((min, max)) = int_range(n) {
            return TypeSpec::Int { min, max };
        }
        match n {
            "f32" | "f64" | "float" => return TypeSpec::Float,
            "bool" => return TypeSpec::Bool,
            "str" | "string" => return TypeSpec::Str,
            "vec" | "hvec" => return TypeSpec::Hvec,
            "array" => return TypeSpec::Array(Box::new(TypeSpec::Any)),
            "map" => return TypeSpec::Map(Box::new(TypeSpec::Str), Box::new(TypeSpec::Any)),
            _ => {}
        }
        if LENIENT.contains(&n) {
            return TypeSpec::Any;
        }
        if decls.structs.contains_key(n) || decls.enums.contains_key(n) || decls.aliases.contains_key(n) {
            return TypeSpec::Named(n.to_string());
        }
        TypeSpec::Opaque(n.to_string())
    }

    /// Does this spec (or anything inside it) name something no declaration and no built-in covers?
    pub fn opaque_names(&self, out: &mut Vec<String>) {
        match self {
            TypeSpec::Opaque(n) => out.push(n.clone()),
            TypeSpec::Array(t) | TypeSpec::Nullable(t) => t.opaque_names(out),
            TypeSpec::Map(k, v) => { k.opaque_names(out); v.opaque_names(out); }
            TypeSpec::Tuple(ts) | TypeSpec::Union(ts) => ts.iter().for_each(|t| t.opaque_names(out)),
            _ => {}
        }
    }

    pub fn spell(&self) -> String {
        match self {
            TypeSpec::Any => "any".into(),
            TypeSpec::Int { min, max } => match (*min, *max) {
                (m, x) if m == i64::MIN && x == i64::MAX => "i64".into(),
                (0, x) if x == i64::MAX => "u64".into(),
                (m, x) if m == i32::MIN as i64 && x == i32::MAX as i64 => "i32".into(),
                (0, x) if x == u32::MAX as i64 => "u32".into(),
                (m, x) if m == i16::MIN as i64 && x == i16::MAX as i64 => "i16".into(),
                (0, x) if x == u16::MAX as i64 => "u16".into(),
                (m, x) if m == i8::MIN as i64 && x == i8::MAX as i64 => "i8".into(),
                (0, x) if x == u8::MAX as i64 => "u8".into(),
                (m, x) => format!("int[{}..{}]", m, x),
            },
            TypeSpec::Float => "f64".into(),
            TypeSpec::Bool => "bool".into(),
            TypeSpec::Str => "str".into(),
            TypeSpec::Hvec => "vec".into(),
            TypeSpec::Array(t) => format!("array<{}>", t.spell()),
            TypeSpec::Map(k, v) => format!("map<{}, {}>", k.spell(), v.spell()),
            TypeSpec::Tuple(ts) => format!("tuple<{}>", ts.iter().map(|t| t.spell()).collect::<Vec<_>>().join(", ")),
            TypeSpec::Union(ts) => ts.iter().map(|t| t.spell()).collect::<Vec<_>>().join(" | "),
            TypeSpec::Nullable(t) => format!("{}?", t.spell()),
            TypeSpec::Named(n) | TypeSpec::Opaque(n) => n.clone(),
        }
    }

    /// Run-time check. `Ok` or the reason the value is not of this type.
    pub fn check(&self, v: &Value, decls: &TypeTable) -> Result<(), String> {
        self.check_depth(v, decls, 0)
    }

    fn check_depth(&self, v: &Value, decls: &TypeTable, depth: u32) -> Result<(), String> {
        if matches!(v, Value::Null) || depth > 32 {
            return Ok(());
        }
        let fail = || Err(format!("{} is not a {}", describe(v), self.spell()));
        match self {
            TypeSpec::Any | TypeSpec::Opaque(_) => Ok(()),
            TypeSpec::Int { min, max } => {
                let n = match v {
                    Value::Int(n) => *n,
                    Value::Float(f) if f.fract() == 0.0 && f.is_finite() && f.abs() < 9.0e18 => *f as i64,
                    _ => return fail(),
                };
                if n < *min || n > *max {
                    return Err(format!("{} is out of range for {}", n, self.spell()));
                }
                Ok(())
            }
            TypeSpec::Float => match v { Value::Int(_) | Value::Float(_) => Ok(()), _ => fail() },
            TypeSpec::Bool => match v { Value::Bool(_) | Value::Int(0) | Value::Int(1) => Ok(()), _ => fail() },
            TypeSpec::Str => match v { Value::Str(_) => Ok(()), _ => fail() },
            TypeSpec::Hvec => match v { Value::Hvec(_) => Ok(()), _ => fail() },
            TypeSpec::Array(t) => match v {
                Value::Array(items) => {
                    for (i, it) in items.iter().enumerate() {
                        t.check_depth(it, decls, depth + 1).map_err(|e| format!("element {}: {}", i, e))?;
                    }
                    Ok(())
                }
                _ => fail(),
            },
            TypeSpec::Map(_, val) => match v {
                Value::Map(m) => {
                    for (k, it) in m {
                        val.check_depth(it, decls, depth + 1).map_err(|e| format!("[{:?}]: {}", k, e))?;
                    }
                    Ok(())
                }
                _ => fail(),
            },
            TypeSpec::Tuple(ts) => match v {
                Value::Array(items) if items.len() == ts.len() => {
                    for (i, (t, it)) in ts.iter().zip(items).enumerate() {
                        t.check_depth(it, decls, depth + 1).map_err(|e| format!("element {}: {}", i, e))?;
                    }
                    Ok(())
                }
                _ => fail(),
            },
            TypeSpec::Union(ts) => {
                if ts.iter().any(|t| t.check_depth(v, decls, depth + 1).is_ok()) { Ok(()) } else { fail() }
            }
            TypeSpec::Nullable(t) => t.check_depth(v, decls, depth + 1),
            TypeSpec::Named(n) => {
                if let Some(alias) = decls.aliases.get(n) {
                    return alias.check_depth(v, decls, depth + 1);
                }
                if let Some(variants) = decls.enums.get(n) {
                    return match v {
                        Value::Str(s) if variants.iter().any(|x| x == s) => Ok(()),
                        Value::Int(_) => Ok(()),
                        _ => Err(format!("{} is not a variant of {} ({})", describe(v), n, variants.join(", "))),
                    };
                }
                let Some(def) = decls.structs.get(n) else { return Ok(()) };
                let Value::Map(m) = v else { return fail() };
                for (field, ty, has_default) in &def.fields {
                    match m.get(field) {
                        Some(fv) => ty.check_depth(fv, decls, depth + 1).map_err(|e| format!("{}.{}: {}", n, field, e))?,
                        None if *has_default => {}
                        None => return Err(format!("{} is missing field {}", n, field)),
                    }
                }
                if let Some(extra) = m.keys().find(|k| !def.fields.iter().any(|(f, _, _)| f == *k)) {
                    return Err(format!("{} has no field {}", n, extra));
                }
                Ok(())
            }
        }
    }

    /// The spec a literal expression has at compile time, when it is a literal at all.
    pub fn of_literal(e: &crate::ast::Expr) -> Option<TypeSpec> {
        use crate::ast::Expr;
        Some(match e {
            Expr::IntLit(n) => TypeSpec::Int { min: *n, max: *n },
            Expr::FloatLit(_) => TypeSpec::Float,
            Expr::StringLit(_) => TypeSpec::Str,
            Expr::BoolLit(_) => TypeSpec::Bool,
            Expr::StructLit(name, _) => TypeSpec::Named(name.clone()),
            _ => return None,
        })
    }

    /// Compile-time compatibility of a literal's spec with a declared spec.
    pub fn accepts_literal(&self, lit: &TypeSpec, decls: &TypeTable) -> bool {
        match self {
            TypeSpec::Any | TypeSpec::Opaque(_) => true,
            TypeSpec::Nullable(t) => t.accepts_literal(lit, decls),
            TypeSpec::Union(ts) => ts.iter().any(|t| t.accepts_literal(lit, decls)),
            TypeSpec::Named(n) if decls.aliases.contains_key(n) => decls.aliases[n].accepts_literal(lit, decls),
            TypeSpec::Named(n) if decls.enums.contains_key(n) => matches!(lit, TypeSpec::Str | TypeSpec::Int { .. }),
            _ => match (self, lit) {
                (TypeSpec::Int { min, max }, TypeSpec::Int { min: v, .. }) => v >= min && v <= max,
                (TypeSpec::Float, TypeSpec::Int { .. } | TypeSpec::Float) => true,
                (TypeSpec::Bool, TypeSpec::Bool) => true,
                (TypeSpec::Bool, TypeSpec::Int { min, .. }) => *min == 0 || *min == 1,
                (TypeSpec::Str, TypeSpec::Str) => true,
                (TypeSpec::Named(a), TypeSpec::Named(b)) => a == b,
                _ => false,
            },
        }
    }
}

/// A short description of a value for an error message.
pub fn describe(v: &Value) -> String {
    match v {
        Value::Int(n) => format!("int {}", n),
        Value::Float(f) => format!("float {}", f),
        Value::Str(s) => format!("string {:?}", if s.len() > 40 { &s[..40] } else { s }),
        Value::Bool(b) => format!("bool {}", b),
        Value::Null => "null".into(),
        Value::Array(a) => format!("an array of {}", a.len()),
        Value::Map(m) => format!("a map of {} field(s)", m.len()),
        Value::Hvec(_) => "a hypervector".into(),
    }
}

/// Every struct a source file declares: at the top level and inside containers.
pub fn all_structs(source: &crate::ast::SourceFile) -> Vec<&crate::ast::StructDecl> {
    use crate::ast::{ContainerItem, TopLevel};
    let mut out = Vec::new();
    for item in &source.items {
        match item {
            TopLevel::Struct(s) => out.push(s),
            TopLevel::Container(c) => {
                for ci in &c.body {
                    if let ContainerItem::Struct(s) = ci {
                        out.push(s);
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// The declarations a source file carries, as a `TypeTable` (aliases resolved against the rest).
pub fn table_of(source: &crate::ast::SourceFile) -> TypeTable {
    use crate::ast::TopLevel;
    let mut t = TypeTable::default();
    let structs = all_structs(source);
    // names first, so a struct field can name another struct declared later in the file
    for s in &structs {
        t.structs.insert(s.name.clone(), StructDef { name: s.name.clone(), fields: Vec::new() });
    }
    for item in &source.items {
        match item {
            TopLevel::Enum(e) => { t.enums.insert(e.name.clone(), e.variants.iter().map(|v| v.name.clone()).collect()); }
            TopLevel::TypeAlias(a) => { t.aliases.insert(a.name.clone(), TypeSpec::Any); }
            _ => {}
        }
    }
    let names = t.clone();
    for s in &structs {
        let fields = s.fields.iter()
            .map(|f| (f.name.clone(), TypeSpec::from_expr(&f.ty, &names), f.default.is_some()))
            .collect();
        t.structs.insert(s.name.clone(), StructDef { name: s.name.clone(), fields });
    }
    for item in &source.items {
        if let TopLevel::TypeAlias(a) = item {
            t.aliases.insert(a.name.clone(), TypeSpec::from_expr(&a.ty, &names));
        }
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap as Map;

    fn point() -> TypeTable {
        let mut t = TypeTable::default();
        t.structs.insert("Point".into(), StructDef { name: "Point".into(), fields: vec![
            ("x".into(), TypeSpec::Float, false),
            ("y".into(), TypeSpec::Float, false),
            ("label".into(), TypeSpec::Str, true),
        ]});
        t.enums.insert("Tier".into(), vec!["easy".into(), "hard".into()]);
        t
    }

    #[test]
    fn null_is_every_type_and_lenient_names_check_nothing() {
        let t = TypeTable::default();
        for name in ["i64", "str", "bool", "Point"] {
            assert!(TypeSpec::from_name(name, &t).check(&Value::Null, &t).is_ok(), "{}", name);
        }
        assert_eq!(TypeSpec::from_name("number", &t), TypeSpec::Any);
        assert_eq!(TypeSpec::from_name("Input", &t), TypeSpec::Opaque("Input".into()));
    }

    #[test]
    fn numbers_widen_never_narrow_and_ranges_hold() {
        let t = TypeTable::default();
        let i64_ = TypeSpec::from_name("i64", &t);
        assert!(i64_.check(&Value::Float(4.0), &t).is_ok());
        assert!(i64_.check(&Value::Float(3.5), &t).is_err());
        assert!(TypeSpec::Float.check(&Value::Int(3), &t).is_ok());
        assert!(TypeSpec::from_name("u8", &t).check(&Value::Int(300), &t).unwrap_err().contains("out of range"));
        assert!(TypeSpec::from_name("u32", &t).check(&Value::Int(-1), &t).is_err());
        assert!(TypeSpec::Bool.check(&Value::Int(1), &t).is_ok());
        assert!(TypeSpec::Bool.check(&Value::Int(2), &t).is_err());
        assert!(TypeSpec::Str.check(&Value::Int(7), &t).unwrap_err().contains("int 7 is not a str"));
    }

    #[test]
    fn structs_are_structural_with_defaults_and_no_extra_fields() {
        let t = point();
        let p = TypeSpec::Named("Point".into());
        let mut m = Map::new();
        m.insert("x".to_string(), Value::Int(1));
        m.insert("y".to_string(), Value::Float(2.5));
        assert!(p.check(&Value::Map(m.clone()), &t).is_ok(), "label has a default");
        m.insert("z".to_string(), Value::Int(0));
        assert!(p.check(&Value::Map(m.clone()), &t).unwrap_err().contains("no field z"));
        m.remove("z");
        m.remove("y");
        assert!(p.check(&Value::Map(m.clone()), &t).unwrap_err().contains("missing field y"));
        m.insert("y".to_string(), Value::Str("two".into()));
        assert!(p.check(&Value::Map(m), &t).unwrap_err().contains("Point.y"));
        assert!(TypeSpec::Named("Tier".into()).check(&Value::Str("easy".into()), &t).is_ok());
        assert!(TypeSpec::Named("Tier".into()).check(&Value::Str("medium".into()), &t).is_err());
    }

    #[test]
    fn literals_against_declarations() {
        let t = point();
        assert!(TypeSpec::from_name("u8", &t).accepts_literal(&TypeSpec::Int { min: 7, max: 7 }, &t));
        assert!(!TypeSpec::from_name("u8", &t).accepts_literal(&TypeSpec::Int { min: -1, max: -1 }, &t));
        assert!(!TypeSpec::Str.accepts_literal(&TypeSpec::Int { min: 1, max: 1 }, &t));
        assert!(TypeSpec::Float.accepts_literal(&TypeSpec::Int { min: 1, max: 1 }, &t));
        assert!(TypeSpec::Nullable(Box::new(TypeSpec::Str)).accepts_literal(&TypeSpec::Str, &t));
    }
}
