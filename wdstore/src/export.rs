//! An entity as stored, written as one line of JSON (`wdstore dump`): what a second, independent reader of the
//! dump's Turtle is compared against (the order's round-trip acceptance). Statement order is the store's (property,
//! statement id); qualifiers, their value nodes and references are listed in the store's canonical order.

use crate::build::j;
use crate::model::*;

pub fn value(v: &Value) -> String {
    match v {
        Value::Entity(e) => format!("{{\"entity\": {}}}", j(&e.text())),
        Value::Str(s) => format!("{{\"str\": {}}}", j(s)),
        Value::Iri(s) => format!("{{\"iri\": {}}}", j(s)),
        Value::Mono { text, lang } => format!("{{\"mono\": [{}, {}]}}", j(text), j(lang)),
        Value::Typed { lit, dtype } => format!("{{\"typed\": [{}, {}]}}", j(lit), j(dtype)),
        Value::SomeValue => "\"somevalue\"".into(),
        Value::NoValue => "\"novalue\"".into(),
    }
}

fn snaks(s: &[Snak]) -> String { s.iter().map(|x| format!("[{}, {}]", x.pid, value(&x.value))).collect::<Vec<_>>().join(", ") }
fn nodes(n: &[(u32, NodeHash)]) -> String {
    n.iter().map(|(p, h)| format!("[{p}, {}]", j(&String::from_utf8_lossy(&h.0)))).collect::<Vec<_>>().join(", ")
}

pub fn entity(e: &Entity, cold: bool) -> String {
    let names: Vec<String> = e.names.iter().map(|n| format!("[{}, {}, {}]", j(&n.lang), n.kind as u8, j(&n.text))).collect();
    let sites: Vec<String> = e.sitelinks.iter().map(|(s, t)| format!("[{}, {}]", j(s), j(t))).collect();
    let st: Vec<String> = e.statements.iter().map(|s| format!(
        concat!("{{\"guid\": {}, \"pid\": {}, \"rank\": {}, \"best\": {}, \"value\": {}, \"value_node\": {}, ",
                "\"hot_qualifiers\": [{}], \"hot_qualifier_nodes\": [{}], \"cold_qualifiers\": [{}], \"cold_qualifier_nodes\": [{}], ",
                "\"references\": [{}]}}"),
        j(&s.guid), s.pid, s.rank as u8, s.best, value(&s.value),
        s.value_node.as_ref().map(|h| j(&String::from_utf8_lossy(&h.0))).unwrap_or("null".into()),
        snaks(&s.hot_qualifiers), nodes(&s.hot_qualifier_nodes), snaks(&s.cold_qualifiers), nodes(&s.cold_qualifier_nodes),
        s.references.iter().map(|h| j(&String::from_utf8_lossy(&h.0))).collect::<Vec<_>>().join(", "))).collect();
    format!("{{\"id\": {}, \"cold\": {cold}, \"revision\": {}, \"modified\": {}, \"datatype\": {}, \"names\": [{}], \"sitelinks\": [{}], \"statements\": [{}]}}",
        j(&e.id.map(|i| i.text()).unwrap_or_default()), e.revision, j(&e.modified),
        e.datatype.as_ref().map(|d| j(d)).unwrap_or("null".into()), names.join(", "), sites.join(", "), st.join(", "))
}
