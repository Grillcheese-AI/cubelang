//! `wdstore serve <dir>`: the store answering one JSON request a line on stdin, one JSON answer a line on stdout (a
//! pipe, not a socket: the reader that starts it hands nothing to the network). Requests:
//!   {"op": "snapshot"}                                   the state served (`snapshot`)
//!   {"op": "get", "id": "Q42", "as_of": "2019", "snapshot": "<hash>"}
//!        the entity as stored (a redirect followed, said); with `as_of` (YYYY, YYYY-MM or YYYY-MM-DD, Gregorian),
//!        each statement says whether it holds then: holds, does_not, too_coarse, undated. With `snapshot`, a store
//!        in another state answers {"error": "snapshot_not_held"} and nothing else.
//!   {"op": "name", "text": "Douglas Adams"}             every hot entity the name may mean, in id order
//!   {"op": "who", "pid": 31, "value": "Q5", "limit": 1000}   the first hot entities with that main value (ranks),
//!                                                        and how many there are in all ("total")
//!   {"op": "pointing_at", "value": "Q5", "limit": 1000}  the first hot entities pointing at it, whatever the
//!                                                        property, and how many there are in all
//!   {"op": "resolve", "id": "Q7"}                        the id that remains
//!   {"op": "labels", "ids": ["Q5", "P31"], "lang": "en"} each entity's label in that language (absent: none)
//! The store never reads a clock: a date is always the request's.

use std::io::{BufRead, Write};

use serde_json::{json, Value as J};

use crate::asof::{holds, span, Holds};
use crate::build::j;
use crate::model::*;
use crate::names::{Found, How};
use crate::store::Store;

fn asked(s: &str) -> Option<(i64, i64)> {
    let parts: Vec<&str> = s.split('-').collect();
    let (lit, p) = match parts.len() {
        1 => (format!("{}-01-01T00:00:00Z", parts[0]), 9),
        2 => (format!("{}-{}-01T00:00:00Z", parts[0], parts[1]), 10),
        3 => (format!("{s}T00:00:00Z"), 11),
        _ => return None,
    };
    span(&lit, p, "Q1985727")
}

/// A request's `limit` (who, pointing_at): at most this many hits come back, with the total beside them.
fn limit(r: &J) -> usize { r.get("limit").and_then(J::as_u64).map(|x| x as usize).unwrap_or(1000).min(100_000) }

fn label(st: &Store, id: Eid, lang: &str) -> Result<Option<String>, String> {
    Ok(st.get(id)?.and_then(|e| e.names.into_iter().find(|n| n.lang == lang && n.kind == NameKind::Label).map(|n| n.text)))
}

fn answer(st: &Store, snap: &str, r: &J) -> Result<J, String> {
    let want = r.get("snapshot").and_then(J::as_str);
    if want.is_some_and(|w| w != snap) { return Ok(json!({"error": "snapshot_not_held", "served": snap})); }
    let id = |k: &str| r.get(k).and_then(J::as_str).and_then(Eid::parse).ok_or(format!("{k}: an id"));
    Ok(match r.get("op").and_then(J::as_str).unwrap_or("") {
        "snapshot" => json!({"snapshot": snap}),
        "resolve" => json!({"id": st.resolve(id("id")?)?.text()}),
        "get" => {
            let asked_id = id("id")?;
            let to = st.resolve(asked_id)?;
            let Some((cold, e, nodes)) = st.locate_with_nodes(to)? else { return Ok(json!({"found": false, "id": to.text()})) };
            let mut out: J = serde_json::from_str(&crate::export::entity(&e, cold)).map_err(|x| x.to_string())?;
            if to != asked_id { out["redirected_from"] = json!(asked_id.text()); }
            if let Some(d) = r.get("as_of").and_then(J::as_str) {
                let a = asked(d).ok_or(format!("as_of: {d}"))?;
                let find = |h: &NodeHash| nodes.get(h).cloned();
                for (k, s) in e.statements.iter().enumerate() {
                    out["statements"][k]["holds"] = json!(match holds(s, a, &find) {
                        Holds::Holds => "holds", Holds::DoesNot => "does_not", Holds::TooCoarse => "too_coarse", Holds::Undated => "undated" });
                }
                out["as_of"] = json!(d);
            }
            out["found"] = json!(true);
            out
        }
        "name" => match st.name(r.get("text").and_then(J::as_str).ok_or("text")?)? {
            Found::Hits(h) => json!({"hits": h.iter().map(|x| json!({"id": x.entity.text(), "lang": x.lang, "kind": x.kind as u8,
                "how": match x.how { How::Exact => "exact".to_string(), How::Folded => "folded".into(), How::Typo(d) => format!("typo{d}") },
                "key": x.key})).collect::<Vec<_>>()}),
            Found::TooMany(n) => json!({"too_many": n}),
            Found::None => json!({"hits": []}),
        },
        "who" => {
            let pid = r.get("pid").and_then(J::as_u64).ok_or("pid")? as u32;
            let (total, hits) = st.who_first(pid, id("value")?, limit(r))?;
            json!({"total": total, "hits": hits.iter().map(|h| json!([h.subject.text(), h.rank as u8, h.best])).collect::<Vec<_>>()})
        }
        "pointing_at" => {
            let (total, hits) = st.pointing_at_first(id("value")?, limit(r))?;
            json!({"total": total, "hits": hits.iter().map(|(p, h)| json!([p, h.subject.text(), h.rank as u8, h.best])).collect::<Vec<_>>()})
        }
        "labels" => {
            let lang = r.get("lang").and_then(J::as_str).unwrap_or("en");
            let mut m = serde_json::Map::new();
            for x in r.get("ids").and_then(J::as_array).ok_or("ids")? {
                if let Some(e) = x.as_str().and_then(Eid::parse) { if let Some(l) = label(st, e, lang)? { m.insert(e.text(), json!(l)); } }
            }
            J::Object(m)
        }
        op => return Err(format!("unknown op {}", j(op))),
    })
}

/// Answers requests from `input` on `output` until the input ends; an error is an answer ({"error": ...}), never an exit.
pub fn serve(dir: &str, input: impl BufRead, mut output: impl Write) -> Result<(), String> {
    let st = Store::open(dir)?;
    let snap = crate::snapshot::at(dir, None)?.hash;
    for line in input.lines() {
        let line = line.map_err(|e| e.to_string())?;
        if line.trim().is_empty() { continue; }
        let a = serde_json::from_str::<J>(&line).map_err(|e| e.to_string()).and_then(|r| answer(&st, &snap, &r))
            .unwrap_or_else(|e| json!({"error": e}));
        writeln!(output, "{a}").and_then(|_| output.flush()).map_err(|e| e.to_string())?;
    }
    Ok(())
}
