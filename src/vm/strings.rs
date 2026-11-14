//! Fix list P6-5 and P6-7 (2026-11-13): whole-number division and its remainder, string helpers, and `+` on strings.
//!
//! - `whole_div`: DIV with a third operand (1 = floor division `a // b`, 2 = remainder `a % b`), so that
//!   `a == (a // b) * b + a % b` and the remainder takes the divisor's sign (-7 // 2 is -4, -7 % 2 is 1). DIV without
//!   it keeps its meaning (true division: 7 / 2 is 3.5). No opcode byte changes.
//! - `concat`: ADD on two strings joins them. A string and anything else fails with `TypeMismatch` (it used to be
//!   read as a number, silently).
//! - `helper`: `starts_with`, `ends_with`, `contains` (two strings, 1 or 0) and `upper`, `lower` (one string),
//!   called as `f(s, ...)` or `s.f(...)`. They are the VM's last resort for a call: a program's own function, or a
//!   `use`d module's, of the same name is called instead. Characters are Unicode scalar values, as `s[i]` and `len`
//!   count them, and case follows Unicode (`"été"` upper is `"ÉTÉ"`).
use super::engine::Value;

/// The VM's string helpers, by name: `None` when `name` is not one of them.
pub fn helper(name: &str, args: &[Value]) -> Option<Result<Value, String>> {
    let two = |f: fn(&str, &str) -> bool| -> Result<Value, String> {
        match args {
            [Value::Str(s), Value::Str(p)] => Ok(Value::Int(f(s, p) as i64)),
            [a, b] => Err(format!("TypeMismatch: {} takes two strings, not {} and {}", name, kind(a), kind(b))),
            _ => Err(format!("{} takes two arguments, not {}", name, args.len())),
        }
    };
    let one = |f: fn(&str) -> String| -> Result<Value, String> {
        match args {
            [Value::Str(s)] => Ok(Value::Str(f(s))),
            [a] => Err(format!("TypeMismatch: {} takes a string, not {}", name, kind(a))),
            _ => Err(format!("{} takes one argument, not {}", name, args.len())),
        }
    };
    Some(match name {
        "starts_with" => two(|s, p| s.starts_with(p)),
        "ends_with" => two(|s, p| s.ends_with(p)),
        "contains" => two(|s, p| s.contains(p)),
        "upper" => one(|s| s.to_uppercase()),
        "lower" => one(|s| s.to_lowercase()),
        _ => return None,
    })
}

/// ADD on strings: `None` when neither side is a string (the numeric path).
pub fn concat(cur: &Value, rhs: &Value) -> Option<Result<Value, String>> {
    match (cur, rhs) {
        (Value::Str(a), Value::Str(b)) => Some(Ok(Value::Str(format!("{}{}", a, b)))),
        (Value::Str(_), _) | (_, Value::Str(_)) => Some(Err(format!(
            "TypeMismatch: `+` joins two strings or adds two numbers, not {} and {}", kind(cur), kind(rhs)))),
        _ => None,
    }
}

/// DIV's third operand: 1 = floor division, 2 = its remainder (`mode` as the compiler's `lower` emits it).
pub fn whole_div(x: &Value, y: &Value, mode: i64) -> Result<Value, String> {
    let rem = match mode {
        1 => false,
        2 => true,
        m => return Err(format!("DIV: unknown mode {}", m)),
    };
    match (x, y) {
        (Value::Int(a), Value::Int(b)) => {
            if *b == 0 {
                return Err("division by zero".into());
            }
            let mut q = a.checked_div(*b).ok_or_else(|| "IntegerOverflow: whole-number division".to_string())?;
            if a % b != 0 && ((*a < 0) != (*b < 0)) {
                q -= 1;
            }
            Ok(Value::Int(if rem { a - q * b } else { q }))
        }
        (Value::Int(_) | Value::Float(_), Value::Int(_) | Value::Float(_)) => {
            let (a, b) = (x.as_f64(), y.as_f64());
            if b == 0.0 {
                return Err("division by zero".into());
            }
            let q = (a / b).floor();
            let r = if rem { a - q * b } else { q };
            Ok(if r.fract() == 0.0 && r.is_finite() { Value::Int(r as i64) } else { Value::Float(r) })
        }
        _ => Err(format!("TypeMismatch: `{}` takes two numbers, not {} and {}",
            if rem { "%" } else { "//" }, kind(x), kind(y))),
    }
}

fn kind(v: &Value) -> &'static str {
    match v {
        Value::Int(_) | Value::Float(_) => "a number",
        Value::Str(_) => "a string",
        Value::Bool(_) => "a boolean",
        Value::Null => "a missing value (null)",
        Value::Array(_) => "a list",
        Value::Map(_) => "a map",
        Value::Hvec(_) => "a vector",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_division_and_remainder_agree() {
        for (a, b) in [(7, 2), (-7, 2), (7, -2), (-7, -2), (6, 3), (0, 5)] {
            let q = whole_div(&Value::Int(a), &Value::Int(b), 1).unwrap().as_i64();
            let r = whole_div(&Value::Int(a), &Value::Int(b), 2).unwrap().as_i64();
            assert_eq!(q * b + r, a, "{} // {} = {}, % = {}", a, b, q, r);
            assert!(r == 0 || (r < 0) == (b < 0), "the remainder takes the divisor's sign: {} % {} = {}", a, b, r);
        }
    }
}
