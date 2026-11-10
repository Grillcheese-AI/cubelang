//! A chunk file: entities sorted by id, each a self-contained record, behind an index (id -> offset) so one entity
//! is read without the rest; then the value nodes, reference nodes and redirects the chunk's input wrote.
//!
//!   "WDSTORE\0" | u32 version | u64 dict_at | u64 index_at | u64 nodes_at | u64 refs_at | u64 redirects_at
//!   entity records ... | dictionary | index | value nodes | reference nodes | redirects
//!
//! Recorded time (the base's side of the two clocks) is each entity's revision and timestamp, with the dump's date
//! in the build manifest; valid time is the hot qualifiers. An overlay closes a statement with a recorded end; the
//! base holds none (every base statement is open).

use crate::codec::{Dict, R, W};
use crate::model::*;

pub const MAGIC: &[u8; 8] = b"WDSTORE\0";
pub const VERSION: u32 = 1;
const HEAD: usize = 8 + 4 + 8 * 5;

#[derive(Default, Debug, PartialEq)]
pub struct Chunk {
    pub entities: Vec<Entity>,
    pub value_nodes: Vec<ValueNode>,
    pub ref_nodes: Vec<RefNode>,
    pub redirects: Vec<(Eid, Eid)>,
}

fn put_value(w: &mut W, d: &mut Dict, v: &Value) {
    match v {
        Value::Entity(e) => { w.byte(0); w.u(e.0); }
        Value::Str(s) => { w.byte(1); w.s(s); }
        Value::Iri(s) => { w.byte(2); w.s(s); }
        Value::Mono { text, lang } => { w.byte(3); w.s(text); let l = d.id(lang); w.u(l); }
        Value::Typed { lit, dtype } => { w.byte(4); w.s(lit); let t = d.id(dtype); w.u(t); }
        Value::SomeValue => w.byte(5),
        Value::NoValue => w.byte(6),
    }
}

fn get_value(r: &mut R, d: &[String]) -> Result<Value, String> {
    let dict = |i: u64| d.get(i as usize).cloned().ok_or_else(|| format!("dictionary entry {i}"));
    Ok(match r.byte()? {
        0 => Value::Entity(Eid(r.u()?)),
        1 => Value::Str(r.s()?),
        2 => Value::Iri(r.s()?),
        3 => { let text = r.s()?; Value::Mono { text, lang: dict(r.u()?)? } }
        4 => { let lit = r.s()?; Value::Typed { lit, dtype: dict(r.u()?)? } }
        5 => Value::SomeValue,
        6 => Value::NoValue,
        t => return Err(format!("value tag {t}")),
    })
}

fn put_snaks(w: &mut W, d: &mut Dict, s: &[Snak]) { w.u(s.len() as u64); for x in s { w.u(x.pid as u64); put_value(w, d, &x.value); } }
fn get_snaks(r: &mut R, d: &[String]) -> Result<Vec<Snak>, String> {
    (0..r.u()?).map(|_| Ok(Snak { pid: r.u()? as u32, value: get_value(r, d)? })).collect()
}
fn put_nodes(w: &mut W, n: &[(u32, NodeHash)]) { w.u(n.len() as u64); for (p, h) in n { w.u(*p as u64); w.hash(h); } }
fn get_nodes(r: &mut R) -> Result<Vec<(u32, NodeHash)>, String> { (0..r.u()?).map(|_| Ok((r.u()? as u32, r.hash()?))).collect() }

fn put_entity(w: &mut W, d: &mut Dict, e: &Entity) {
    w.u(e.revision); w.s(&e.modified);
    match &e.datatype { Some(t) => { w.byte(1); let i = d.id(t); w.u(i); } None => w.byte(0) }
    w.u(e.names.len() as u64);
    for n in &e.names { let l = d.id(&n.lang); w.u(l); w.byte(n.kind as u8); w.s(&n.text); }
    w.u(e.sitelinks.len() as u64);
    for (site, t) in &e.sitelinks { let i = d.id(site); w.u(i); w.s(t); }
    w.u(e.statements.len() as u64);
    for s in &e.statements {
        w.u(s.pid as u64); w.s(&s.guid);
        w.byte((s.rank as u8) | if s.best { 4 } else { 0 } | if s.value_node.is_some() { 8 } else { 0 });
        put_value(w, d, &s.value);
        if let Some(h) = &s.value_node { w.hash(h); }
        put_snaks(w, d, &s.hot_qualifiers); put_nodes(w, &s.hot_qualifier_nodes);
        put_snaks(w, d, &s.cold_qualifiers); put_nodes(w, &s.cold_qualifier_nodes);
        w.u(s.references.len() as u64); for h in &s.references { w.hash(h); }
    }
}

fn get_entity(r: &mut R, d: &[String], id: Eid) -> Result<Entity, String> {
    let dict = |i: u64| d.get(i as usize).cloned().ok_or_else(|| format!("dictionary entry {i}"));
    let mut e = Entity { id: Some(id), revision: r.u()?, modified: r.s()?, ..Default::default() };
    if r.byte()? == 1 { e.datatype = Some(dict(r.u()?)?); }
    for _ in 0..r.u()? {
        let lang = dict(r.u()?)?;
        let kind = match r.byte()? { 0 => NameKind::Label, 1 => NameKind::Alias, 2 => NameKind::Description, k => return Err(format!("name kind {k}")) };
        e.names.push(Name { lang, kind, text: r.s()? });
    }
    for _ in 0..r.u()? { let site = dict(r.u()?)?; e.sitelinks.push((site, r.s()?)); }
    for _ in 0..r.u()? {
        let pid = r.u()? as u32; let guid = r.s()?; let f = r.byte()?;
        let rank = match f & 3 { 0 => Rank::Deprecated, 1 => Rank::Normal, 2 => Rank::Preferred, _ => return Err("rank".into()) };
        let value = get_value(r, d)?;
        let value_node = if f & 8 != 0 { Some(r.hash()?) } else { None };
        e.statements.push(Statement { pid, guid, rank, best: f & 4 != 0, value, value_node,
            hot_qualifiers: get_snaks(r, d)?, hot_qualifier_nodes: get_nodes(r)?,
            cold_qualifiers: get_snaks(r, d)?, cold_qualifier_nodes: get_nodes(r)?,
            references: (0..r.u()?).map(|_| r.hash()).collect::<Result<_, _>>()? });
    }
    Ok(e)
}

/// The chunk's bytes. Entities must be sorted by id, with no id twice.
pub fn write(c: &Chunk) -> Result<Vec<u8>, String> {
    let mut d = Dict::default();
    let mut w = W { b: vec![0; HEAD] };
    let mut index = Vec::with_capacity(c.entities.len());
    let mut last: Option<Eid> = None;
    for e in &c.entities {
        let id = e.id.ok_or("an entity without an id")?;
        if last.is_some_and(|l| l >= id) { return Err(format!("entities out of order at {}", id.text())); }
        last = Some(id);
        index.push((id, w.b.len() as u64));
        put_entity(&mut w, &mut d, e);
    }
    let mut tail = W::default();
    for n in &c.value_nodes {
        tail.hash(&n.hash); let k = d.id(&n.kind); tail.u(k); tail.u(n.fields.len() as u64);
        for (f, v) in &n.fields { let i = d.id(f); tail.u(i); tail.s(v); }
    }
    let nodes = std::mem::take(&mut tail.b);
    for n in &c.ref_nodes { tail.hash(&n.hash); put_snaks(&mut tail, &mut d, &n.snaks); put_nodes(&mut tail, &n.nodes); }
    let refs = std::mem::take(&mut tail.b);
    let dict_at = w.b.len() as u64;
    w.u(d.list.len() as u64); for s in &d.list { w.s(s); }
    let index_at = w.b.len() as u64;
    w.u(index.len() as u64);
    for (id, off) in &index { w.raw(&id.0.to_le_bytes()); w.raw(&off.to_le_bytes()); }
    let nodes_at = w.b.len() as u64; w.u(c.value_nodes.len() as u64); w.raw(&nodes);
    let refs_at = w.b.len() as u64; w.u(c.ref_nodes.len() as u64); w.raw(&refs);
    let redirects_at = w.b.len() as u64;
    w.u(c.redirects.len() as u64); for (a, b) in &c.redirects { w.u(a.0); w.u(b.0); }
    let mut head = Vec::with_capacity(HEAD);
    head.extend_from_slice(MAGIC); head.extend_from_slice(&VERSION.to_le_bytes());
    for x in [dict_at, index_at, nodes_at, refs_at, redirects_at] { head.extend_from_slice(&x.to_le_bytes()); }
    w.b[..HEAD].copy_from_slice(&head);
    Ok(w.b)
}

pub struct Reader<'a> { b: &'a [u8], at: [usize; 5], dict: Vec<String>, n: usize, index_body: usize }

impl<'a> Reader<'a> {
    pub fn open(b: &'a [u8]) -> Result<Self, String> {
        if b.len() < HEAD || &b[..8] != MAGIC { return Err("not a store chunk".into()); }
        if u32::from_le_bytes(b[8..12].try_into().unwrap()) != VERSION { return Err("chunk version".into()); }
        let mut at = [0usize; 5];
        for (k, x) in at.iter_mut().enumerate() { *x = u64::from_le_bytes(b[12 + 8 * k..20 + 8 * k].try_into().unwrap()) as usize; }
        let mut r = R { b, i: at[0] };
        let dict = (0..r.u()?).map(|_| r.s()).collect::<Result<_, _>>()?;
        let mut r = R { b, i: at[1] };
        let n = r.u()? as usize;
        Ok(Reader { b, at, dict, n, index_body: r.i })
    }

    pub fn len(&self) -> usize { self.n }
    pub fn is_empty(&self) -> bool { self.n == 0 }

    fn slot(&self, k: usize) -> (Eid, usize) {
        let o = self.index_body + 16 * k;
        (Eid(u64::from_le_bytes(self.b[o..o + 8].try_into().unwrap())),
         u64::from_le_bytes(self.b[o + 8..o + 16].try_into().unwrap()) as usize)
    }

    /// One entity, by binary search on the index; the rest of the chunk is not read.
    pub fn get(&self, id: Eid) -> Result<Option<Entity>, String> {
        let (mut lo, mut hi) = (0usize, self.n);
        while lo < hi {
            let mid = (lo + hi) / 2;
            let (e, off) = self.slot(mid);
            match e.cmp(&id) {
                std::cmp::Ordering::Equal => return get_entity(&mut R { b: self.b, i: off }, &self.dict, e).map(Some),
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
            }
        }
        Ok(None)
    }

    pub fn read_all(&self) -> Result<Chunk, String> {
        let mut c = Chunk::default();
        for k in 0..self.n { let (e, off) = self.slot(k); c.entities.push(get_entity(&mut R { b: self.b, i: off }, &self.dict, e)?); }
        let dict = |i: u64| self.dict.get(i as usize).cloned().ok_or_else(|| format!("dictionary entry {i}"));
        let mut r = R { b: self.b, i: self.at[2] };
        for _ in 0..r.u()? {
            let hash = r.hash()?; let kind = dict(r.u()?)?;
            let fields = (0..r.u()?).map(|_| Ok((dict(r.u()?)?, r.s()?))).collect::<Result<_, String>>()?;
            c.value_nodes.push(ValueNode { hash, kind, fields });
        }
        let mut r = R { b: self.b, i: self.at[3] };
        for _ in 0..r.u()? { let hash = r.hash()?; c.ref_nodes.push(RefNode { hash, snaks: get_snaks(&mut r, &self.dict)?, nodes: get_nodes(&mut r)? }); }
        let mut r = R { b: self.b, i: self.at[4] };
        for _ in 0..r.u()? { c.redirects.push((Eid(r.u()?), Eid(r.u()?))); }
        Ok(c)
    }
}
