//! Valid time, read against the date a question is about (0055's snapshot addendum, section 3): does a statement
//! hold on that date? From its start (P580), end (P582) and point-in-time (P585) qualifiers, each with the
//! precision and calendar its value node gives. Every date is a span of days (a year at year precision is all its
//! days), compared as day numbers; a Julian-calendar date is converted exactly. The question's own date is a span
//! too ("in 2019"). Answers:
//!   Holds        the statement's span covers the whole asked span
//!   DoesNot      the spans do not meet
//!   TooCoarse    they overlap only in part: the dates cannot say (a refusal reason, counted apart)
//!   Undated      no start, end or point in time: valid time unknown (not "always")
//! A P585 statement holds only within its point's span; which of several P585 values to take is the caller's rule.

use crate::model::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Holds { Holds, DoesNot, TooCoarse, Undated }

const GREGORIAN: &str = "Q1985727";
const JULIAN: &str = "Q1985786";

/// Days from a fixed origin (the Julian Day Number) of a date in the Gregorian or Julian calendar.
pub fn day_number(y: i64, m: i64, d: i64, julian: bool) -> i64 {
    let a = (14 - m) / 12;
    let yy = y + 4800 - a;
    let mm = m + 12 * a - 3;
    if julian { d + (153 * mm + 2) / 5 + 365 * yy + yy.div_euclid(4) - 32083 }
    else { d + (153 * mm + 2) / 5 + 365 * yy + yy.div_euclid(4) - yy.div_euclid(100) + yy.div_euclid(400) - 32045 }
}

fn month_days(y: i64, m: i64, julian: bool) -> i64 {
    day_number(if m == 12 { y + 1 } else { y }, if m == 12 { 1 } else { m + 1 }, 1, julian) - day_number(y, m, 1, julian)
}

/// "1952-03-11T00:00:00Z" (a sign allowed; Wikidata writes month or day 00 below their precision) -> (y, m, d).
pub fn parse(lit: &str) -> Option<(i64, i64, i64)> {
    let s = lit.trim_start_matches('+');
    let (neg, s) = match s.strip_prefix('-') { Some(r) => (true, r), None => (false, s) };
    let date = s.split('T').next()?;
    let mut it = date.split('-');
    let y: i64 = it.next()?.parse().ok()?;
    let m: i64 = it.next()?.parse().ok()?;
    let d: i64 = it.next()?.parse().ok()?;
    Some((if neg { -y } else { y }, m, d))
}

/// A date at a precision (Wikidata's: 11 day, 10 month, 9 year, 8 decade, 7 century, 6 millennium, lower ones
/// coarser still) as its span of day numbers, first and last day.
pub fn span(lit: &str, precision: u8, calendar: &str) -> Option<(i64, i64)> {
    let (y, m, d) = parse(lit)?;
    let julian = calendar.ends_with(JULIAN);
    if !julian && !calendar.is_empty() && !calendar.ends_with(GREGORIAN) { return None; }
    let (y0, y1) = match precision {
        11 => return Some((day_number(y, m.max(1), d.max(1), julian), day_number(y, m.max(1), d.max(1), julian))),
        10 => { let first = day_number(y, m.max(1), 1, julian); return Some((first, first + month_days(y, m.max(1), julian) - 1)); }
        9 => (y, y),
        p if p <= 8 => {
            let size = 10i64.pow(9 - p as u32);   // 8: a decade, 7: a century, 6: a millennium, ...
            let base = if p == 8 { y.div_euclid(10) * 10 } else { (y - 1).div_euclid(size) * size + 1 };
            (base, base + size - 1)
        }
        _ => return None,
    };
    Some((day_number(y0, 1, 1, julian), day_number(y1, 12, 31, julian)))
}

/// A time snak's span: its literal, with precision and calendar from its value node (None: the node is not known).
fn snak_span(lit: &str, node: Option<&ValueNode>) -> Option<(i64, i64)> {
    let n = node?;
    let f = |k: &str| n.fields.iter().find(|x| x.0 == k).map(|x| x.1.as_str());
    let p: u8 = f("wikibase:timePrecision")?.parse().ok()?;
    span(f("wikibase:timeValue").unwrap_or(lit), p, f("wikibase:timeCalendarModel").unwrap_or(GREGORIAN))
}

/// Does the statement hold over the asked span (first and last day)? `node` finds a value node by hash.
pub fn holds(st: &Statement, asked: (i64, i64), node: &dyn Fn(&NodeHash) -> Option<ValueNode>) -> Holds {
    let mut bounds: [Vec<(i64, i64)>; 3] = [vec![], vec![], vec![]];   // start, end, point
    let mut unknown = false;
    for q in &st.hot_qualifiers {
        let k = match q.pid { 580 => 0, 582 => 1, 585 => 2, _ => continue };
        match &q.value {
            Value::Typed { lit, .. } => {
                // the qualifier's node: the one of its property whose own time value is this literal
                let n = st.hot_qualifier_nodes.iter().filter(|x| x.0 == q.pid).filter_map(|x| node(&x.1))
                    .find(|v| v.fields.iter().any(|f| f.0 == "wikibase:timeValue" && f.1 == *lit));
                match snak_span(lit, n.as_ref()) { Some(s) => bounds[k].push(s), None => unknown = true }
            }
            Value::SomeValue => unknown = true,     // a start or end that exists but is not known
            _ => {}                                 // no value: no start / no end
        }
    }
    if bounds.iter().all(|b| b.is_empty()) { return if unknown { Holds::TooCoarse } else { Holds::Undated }; }
    if unknown { return Holds::TooCoarse; }
    let (a0, a1) = asked;
    if !bounds[2].is_empty() {
        let (p0, p1) = (bounds[2].iter().map(|x| x.0).min().unwrap(), bounds[2].iter().map(|x| x.1).max().unwrap());
        return if a1 < p0 || a0 > p1 { Holds::DoesNot } else if a0 >= p0 && a1 <= p1 { Holds::Holds } else { Holds::TooCoarse };
    }
    // the statement's span: from its start's first day (none: open) to its end's last day (none: open)
    let (s_lo, s_hi) = (bounds[0].iter().map(|x| x.0).min(), bounds[0].iter().map(|x| x.1).max());
    let (e_lo, e_hi) = (bounds[1].iter().map(|x| x.0).min(), bounds[1].iter().map(|x| x.1).max());
    // surely in force over the asked span: started by its first day, not ended before its last
    let sure = s_hi.is_none_or(|s| s <= a0) && e_lo.is_none_or(|e| e >= a1);
    // surely not: ended before the span begins, or started after it ends
    let never = e_hi.is_some_and(|e| e < a0) || s_lo.is_some_and(|s| s > a1);
    if sure { Holds::Holds } else if never { Holds::DoesNot } else { Holds::TooCoarse }
}
