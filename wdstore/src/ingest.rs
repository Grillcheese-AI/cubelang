//! One entity's block of the dump (its `data:` header to the next) -> what the store keeps: the entity, the value
//! and reference nodes the block writes, and any redirect. Names, descriptions and sitelinks are projected to the
//! store's languages and wikis; statements are kept at every rank, every snak type, every qualifier and reference.

use std::collections::BTreeMap;

use crate::model::*;
use crate::turtle::{triples, Term};

pub struct Params {
    pub langs: Vec<String>,
    pub wikis: Vec<String>,
    /// Qualifier properties kept hot: valid time (start, end, point in time, earliest, latest).
    pub hot_qualifiers: Vec<u32>,
}

impl Default for Params {
    fn default() -> Self {
        Params { langs: vec!["en".into(), "fr".into(), "mul".into()],
                 wikis: vec!["https://en.wikipedia.org/".into(), "https://fr.wikipedia.org/".into()],
                 hot_qualifiers: vec![580, 582, 585, 1319, 1326] }
    }
}

#[derive(Default, Debug)]
pub struct Block {
    pub entity: Option<Entity>,
    pub value_nodes: Vec<ValueNode>,
    pub ref_nodes: Vec<RefNode>,
    pub redirects: Vec<(Eid, Eid)>,
    /// Triples the store does not keep, by predicate (derived ones: wdt:, psn:, the counts on data:, ...).
    pub skipped: BTreeMap<String, u64>,
}

fn name(t: &Term) -> Option<&str> { if let Term::Name(n) = t { Some(n) } else { None } }

fn pid(n: &str, prefix: &str) -> Option<u32> { n.strip_prefix(prefix)?.strip_prefix('P')?.parse().ok() }

pub fn value(t: &Term) -> Value {
    match t {
        Term::Blank(_) => Value::SomeValue,
        Term::Iri(i) if i.starts_with("http://www.wikidata.org/.well-known/genid/") => Value::SomeValue,
        Term::Iri(i) => Value::Iri(i.clone()),
        Term::Name(n) => match n.strip_prefix("wd:").and_then(Eid::parse) {
            Some(e) => Value::Entity(e),
            None => Value::Iri(n.clone()),
        },
        Term::Lit { lex, lang: Some(l), .. } => Value::Mono { text: lex.clone(), lang: l.clone() },
        Term::Lit { lex, dtype: None, .. } => Value::Str(lex.clone()),
        Term::Lit { lex, dtype: Some(d), .. } => Value::Typed { lit: lex.clone(), dtype: d.clone() },
    }
}

fn lit(t: &Term) -> String {
    match t { Term::Lit { lex, .. } => lex.clone(), Term::Name(n) => n.clone(), Term::Iri(i) => i.clone(),
              Term::Blank(b) => format!("_:{b}") }
}

fn hash(n: &str, prefix: &str) -> Option<NodeHash> { n.strip_prefix(prefix).map(|h| NodeHash(h.as_bytes().to_vec())) }

pub fn block(text: &str, prm: &Params) -> Result<Block, String> {
    let mut b = Block::default();
    let mut ent = Entity::default();
    let mut me: Option<String> = None;
    let mut stmt_pid: BTreeMap<String, u32> = BTreeMap::new();
    let mut stmts: BTreeMap<String, Statement> = BTreeMap::new();
    let mut vnodes: BTreeMap<String, ValueNode> = BTreeMap::new();
    let mut rnodes: BTreeMap<String, RefNode> = BTreeMap::new();
    let mut sites: BTreeMap<String, (Option<String>, Option<String>)> = BTreeMap::new();
    let ts = triples(text)?;
    for (s, p, o) in &ts {
        if name(s).is_some_and(|n| n.starts_with("data:")) && name(p) == Some("schema:about") {
            me = name(o).map(|x| x.to_string());
        }
    }
    let skip = |b: &mut Block, p: &str| *b.skipped.entry(p.split(':').next().unwrap_or(p).to_string() + ":" ).or_default() += 1;
    for (s, p, o) in ts {
        let (Some(pn), sn) = (name(&p).map(str::to_string), name(&s).map(str::to_string)) else { skip(&mut b, "iri"); continue };
        match sn.as_deref() {
            Some(x) if x.starts_with("data:") => match pn.as_str() {
                "schema:version" => ent.revision = lit(&o).parse().map_err(|_| format!("revision {o:?}"))?,
                "schema:dateModified" => ent.modified = lit(&o),
                _ => skip(&mut b, &pn),
            },
            Some(x) if Some(x) == me.as_deref() => {
                if pn == "owl:sameAs" {   // a redirect: its own data: header, then `wd:Qold owl:sameAs wd:Qnew`
                    match (x.strip_prefix("wd:").and_then(Eid::parse), name(&o).and_then(|n| n.strip_prefix("wd:")).and_then(Eid::parse)) {
                        (Some(a), Some(c)) => b.redirects.push((a, c)),
                        _ => return Err(format!("redirect {x} -> {o:?}")),
                    }
                    continue;
                }
                if pn == "rdf:type" { if let Some(k) = name(&o) { if k == "wikibase:Item" || k == "wikibase:Property" || k == "ontolex:LexicalEntry" { ent.id = x.strip_prefix("wd:").and_then(Eid::parse); } } continue; }
                let kind = match pn.as_str() { "rdfs:label" => Some(NameKind::Label), "skos:altLabel" => Some(NameKind::Alias),
                    "schema:description" => Some(NameKind::Description), _ => None };
                if let (Some(k), Term::Lit { lex, lang: Some(l), .. }) = (kind, &o) {
                    if prm.langs.iter().any(|x| x == l) { ent.names.push(Name { lang: l.clone(), kind: k, text: lex.clone() }); }
                } else if let Some(pp) = pid(&pn, "p:") {
                    stmt_pid.insert(name(&o).unwrap_or("").to_string(), pp);
                } else if pn == "wikibase:propertyType" {
                    ent.datatype = name(&o).map(str::to_string);
                } else { skip(&mut b, &pn); }
            }
            Some(x) if x.starts_with("wd:") && pn == "owl:sameAs" => {
                match (x.strip_prefix("wd:").and_then(Eid::parse), name(&o).and_then(|n| n.strip_prefix("wd:")).and_then(Eid::parse)) {
                    (Some(a), Some(c)) => b.redirects.push((a, c)),
                    _ => return Err(format!("redirect {x} -> {o:?}")),
                }
            }
            Some(x) if x.starts_with("wds:") => {
                let st = stmts.entry(x.to_string()).or_insert_with(|| Statement { guid: String::new(), pid: 0,
                    rank: Rank::Normal, best: false, value: Value::NoValue, value_node: None, hot_qualifiers: vec![],
                    hot_qualifier_nodes: vec![], cold_qualifiers: vec![], cold_qualifier_nodes: vec![], references: vec![] });
                let on = name(&o).unwrap_or("");
                if pn == "rdf:type" {
                    if on == "wikibase:BestRank" { st.best = true; }
                    else if let Some(q) = pid(on, "wdno:") { st.cold_qualifiers.push(Snak { pid: q, value: Value::NoValue }); }
                } else if pn == "wikibase:rank" {
                    st.rank = match on { "wikibase:PreferredRank" => Rank::Preferred, "wikibase:DeprecatedRank" => Rank::Deprecated,
                        "wikibase:NormalRank" => Rank::Normal, _ => return Err(format!("rank {on}")) };
                } else if let Some(q) = pid(&pn, "ps:") { st.pid = q; st.value = value(&o); }
                else if pid(&pn, "psv:").is_some() { st.value_node = hash(on, "wdv:"); }
                else if let Some(q) = pid(&pn, "pq:") { st.cold_qualifiers.push(Snak { pid: q, value: value(&o) }); }
                else if let Some(q) = pid(&pn, "pqv:") { if let Some(h) = hash(on, "wdv:") { st.cold_qualifier_nodes.push((q, h)); } }
                else if pn == "prov:wasDerivedFrom" { if let Some(h) = hash(on, "wdref:") { st.references.push(h); } }
                else { skip(&mut b, &pn); }
            }
            Some(x) if x.starts_with("wdv:") => {
                let n = vnodes.entry(x.to_string()).or_insert_with(|| ValueNode { hash: hash(x, "wdv:").unwrap(), kind: String::new(), fields: vec![] });
                if pn == "rdf:type" { n.kind = lit(&o); } else { n.fields.push((pn, lit(&o))); }
            }
            Some(x) if x.starts_with("wdref:") => {
                let n = rnodes.entry(x.to_string()).or_insert_with(|| RefNode { hash: hash(x, "wdref:").unwrap(), snaks: vec![], nodes: vec![] });
                if let Some(q) = pid(&pn, "pr:") { n.snaks.push(Snak { pid: q, value: value(&o) }); }
                else if let Some(q) = pid(&pn, "prv:") { if let Some(h) = hash(name(&o).unwrap_or(""), "wdv:") { n.nodes.push((q, h)); } }
                else { skip(&mut b, &pn); }
            }
            _ => match (&s, pn.as_str()) {
                (Term::Iri(article), "schema:isPartOf") => { sites.entry(article.clone()).or_default().0 = Some(lit(&o)); }
                (Term::Iri(article), "schema:name") => { sites.entry(article.clone()).or_default().1 = Some(lit(&o)); }
                _ => skip(&mut b, &pn),
            },
        }
    }
    for (node, mut st) in stmts {
        // a main snak's no value is `a wdno:P` with the statement's own property (a qualifier's has another)
        let own = stmt_pid.get(&node).copied().ok_or_else(|| format!("{node}: no p: link"))?;
        let had_ps = st.pid != 0;
        if had_ps && st.pid != own { return Err(format!("{node}: ps:P{} under p:P{own}", st.pid)); }
        st.pid = own;
        if !had_ps {
            let i = st.cold_qualifiers.iter().position(|q| q.pid == own && q.value == Value::NoValue)
                .ok_or_else(|| format!("{node}: no main snak"))?;
            st.cold_qualifiers.remove(i);
        }
        st.guid = node.splitn(2, '-').nth(1).unwrap_or("").to_string();
        let hot = |p: u32| prm.hot_qualifiers.contains(&p);
        let (h, c): (Vec<_>, Vec<_>) = st.cold_qualifiers.drain(..).partition(|q| hot(q.pid));
        st.hot_qualifiers = h; st.cold_qualifiers = c;
        let (h, c): (Vec<_>, Vec<_>) = st.cold_qualifier_nodes.drain(..).partition(|q| hot(q.0));
        st.hot_qualifier_nodes = h; st.cold_qualifier_nodes = c;
        canon_statement(&mut st);
        ent.statements.push(st);
    }
    for (_, (site, title)) in sites {
        if let (Some(si), Some(t)) = (site, title) { if prm.wikis.contains(&si) { ent.sitelinks.push((si, t)); } }
    }
    ent.names.sort(); ent.names.dedup(); ent.sitelinks.sort(); ent.sitelinks.dedup();
    ent.statements.sort_by(|a, b| (a.pid, &a.guid).cmp(&(b.pid, &b.guid)));
    b.value_nodes = vnodes.into_values().map(|mut n| { n.fields.sort(); n }).collect();
    b.ref_nodes = rnodes.into_values().map(|mut n| { n.snaks.sort_by_key(|s| (s.pid, format!("{:?}", s.value))); n.nodes.sort(); n }).collect();
    if ent.id.is_some() { b.entity = Some(ent); }
    Ok(b)
}

/// One order for what RDF leaves unordered, so a rebuild writes the same bytes; and each triple once: RDF is a set
/// of triples, so a triple the dump writes three times (a statement's same reference, 2026-11-10) means it once.
fn canon_statement(st: &mut Statement) {
    for q in [&mut st.hot_qualifiers, &mut st.cold_qualifiers] { q.sort_by_key(|s| (s.pid, format!("{:?}", s.value))); q.dedup(); }
    for n in [&mut st.hot_qualifier_nodes, &mut st.cold_qualifier_nodes] { n.sort(); n.dedup(); }
    st.references.sort(); st.references.dedup();
}
