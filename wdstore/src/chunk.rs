//! A chunk file (format 2): entities sorted by id, each a self-contained record, behind a fixed-width index
//! (id u64, offset u32) so one entity is read without the rest. Value and reference nodes are named by their number
//! in the store's node files (`nodes.rs`); a chunk holds entities only.
//!
//!   "WDSTORE\0" | u32 version | u64 dict_at | u64 index_at | entity records | dictionary | index
//!
//! A statement: the gap from the previous statement's property, a flags byte (rank: 2 bits; best rank; a main value
//! node; the statement id's kind: 2 bits; has qualifiers; has references), the statement id (16 bytes when a UUID in
//! one case), the main value, then what the flags say is there.
//!
//! Recorded time (the base's side of the two clocks) is each entity's revision and timestamp, with the dump's
//! identity in the manifest; valid time is the hot qualifiers. Every base statement is open; an overlay closes one.

use crate::codec::{entry, guid_kind, Dict, R, W};
use crate::model::*;
use crate::nodes::{Ix, Nodes};

pub const MAGIC: &[u8; 8] = b"WDSTORE\0";
pub const VERSION: u32 = 2;
const HEAD: usize = 8 + 4 + 8 * 2;

pub fn put_value(w: &mut W, d: &mut Dict, v: &Value) {
    match v {
        Value::Entity(e) => { w.byte(0); w.eid(*e); }
        Value::Str(s) => { w.byte(1); w.s(s); }
        Value::Iri(s) => { w.byte(2); w.s(s); }
        Value::Mono { text, lang } => { w.byte(3); w.s(text); let l = d.id(lang); w.u(l); }
        Value::Typed { lit, dtype } => { w.byte(4); w.s(lit); let t = d.id(dtype); w.u(t); }
        Value::SomeValue => w.byte(5),
        Value::NoValue => w.byte(6),
    }
}

pub fn get_value(r: &mut R, d: &[String]) -> Result<Value, String> {
    Ok(match r.byte()? {
        0 => Value::Entity(r.eid()?),
        1 => Value::Str(r.s()?),
        2 => Value::Iri(r.s()?),
        3 => { let text = r.s()?; Value::Mono { text, lang: entry(d, r.u()?)? } }
        4 => { let lit = r.s()?; Value::Typed { lit, dtype: entry(d, r.u()?)? } }
        5 => Value::SomeValue,
        6 => Value::NoValue,
        t => return Err(format!("value tag {t}")),
    })
}

pub fn put_snaks(w: &mut W, d: &mut Dict, s: &[Snak]) {
    w.u(s.len() as u64);
    for x in s { w.u(x.pid as u64); put_value(w, d, &x.value); }
}
pub fn get_snaks(r: &mut R, d: &[String]) -> Result<Vec<Snak>, String> {
    (0..r.u()?).map(|_| Ok(Snak { pid: r.u()? as u32, value: get_value(r, d)? })).collect()
}
pub fn put_nodes(w: &mut W, ix: &Ix, n: &[(u32, NodeHash)]) -> Result<(), String> {
    w.u(n.len() as u64);
    for (p, h) in n { w.u(*p as u64); w.u(ix.value(h)? as u64); }
    Ok(())
}
pub fn get_nodes(r: &mut R, n: &Nodes) -> Result<Vec<(u32, NodeHash)>, String> {
    (0..r.u()?).map(|_| Ok((r.u()? as u32, n.values.hash(r.u()? as u32)?))).collect()
}

fn put_entity(w: &mut W, d: &mut Dict, ix: &Ix, e: &Entity) -> Result<(), String> {
    w.u(e.revision); w.s(&e.modified);
    match &e.datatype { Some(t) => { w.byte(1); let i = d.id(t); w.u(i); } None => w.byte(0) }
    w.u(e.names.len() as u64);
    for n in &e.names { let l = d.id(&n.lang); w.u(l); w.byte(n.kind as u8); w.s(&n.text); }
    w.u(e.sitelinks.len() as u64);
    for (site, t) in &e.sitelinks { let i = d.id(site); w.u(i); w.s(t); }
    w.u(e.statements.len() as u64);
    let mut prev = 0u32;
    for s in &e.statements {
        if s.pid < prev { return Err(format!("{:?}: statements out of property order", e.id)); }
        w.u((s.pid - prev) as u64); prev = s.pid;
        let gk = guid_kind(&s.guid);
        let quals = !(s.hot_qualifiers.is_empty() && s.hot_qualifier_nodes.is_empty() && s.cold_qualifiers.is_empty()
                      && s.cold_qualifier_nodes.is_empty());
        w.byte((s.rank as u8) | (s.best as u8) << 2 | (s.value_node.is_some() as u8) << 3 | gk << 4
               | (quals as u8) << 6 | (!s.references.is_empty() as u8) << 7);
        w.guid(&s.guid, gk);
        put_value(w, d, &s.value);
        if let Some(h) = &s.value_node { w.u(ix.value(h)? as u64); }
        if quals {
            put_snaks(w, d, &s.hot_qualifiers); put_nodes(w, ix, &s.hot_qualifier_nodes)?;
            put_snaks(w, d, &s.cold_qualifiers); put_nodes(w, ix, &s.cold_qualifier_nodes)?;
        }
        if !s.references.is_empty() {
            w.u(s.references.len() as u64);
            for h in &s.references { w.u(ix.reference(h)? as u64); }
        }
    }
    Ok(())
}

fn get_entity(r: &mut R, d: &[String], n: &Nodes, id: Eid) -> Result<Entity, String> {
    let mut e = Entity { id: Some(id), revision: r.u()?, modified: r.s()?, ..Default::default() };
    if r.byte()? == 1 { e.datatype = Some(entry(d, r.u()?)?); }
    for _ in 0..r.u()? {
        let lang = entry(d, r.u()?)?;
        let kind = match r.byte()? { 0 => NameKind::Label, 1 => NameKind::Alias, 2 => NameKind::Description,
                                     k => return Err(format!("name kind {k}")) };
        e.names.push(Name { lang, kind, text: r.s()? });
    }
    for _ in 0..r.u()? { let site = entry(d, r.u()?)?; e.sitelinks.push((site, r.s()?)); }
    let mut pid = 0u32;
    for _ in 0..r.u()? {
        pid += r.u()? as u32;
        let f = r.byte()?;
        let rank = match f & 3 { 0 => Rank::Deprecated, 1 => Rank::Normal, 2 => Rank::Preferred, _ => return Err("rank".into()) };
        let guid = r.guid((f >> 4) & 3)?;
        let value = get_value(r, d)?;
        let value_node = if f & 8 != 0 { Some(n.values.hash(r.u()? as u32)?) } else { None };
        let mut s = Statement { pid, guid, rank, best: f & 4 != 0, value, value_node, hot_qualifiers: vec![],
            hot_qualifier_nodes: vec![], cold_qualifiers: vec![], cold_qualifier_nodes: vec![], references: vec![] };
        if f & 64 != 0 {
            s.hot_qualifiers = get_snaks(r, d)?; s.hot_qualifier_nodes = get_nodes(r, n)?;
            s.cold_qualifiers = get_snaks(r, d)?; s.cold_qualifier_nodes = get_nodes(r, n)?;
        }
        if f & 128 != 0 { s.references = (0..r.u()?).map(|_| n.refs.hash(r.u()? as u32)).collect::<Result<_, _>>()?; }
        e.statements.push(s);
    }
    Ok(e)
}

/// The chunk's bytes. Entities sorted by id, none twice; statements by property.
pub fn write(entities: &[Entity], ix: &Ix) -> Result<Vec<u8>, String> {
    let mut d = Dict::default();
    let mut w = W { b: vec![0; HEAD] };
    let mut index = Vec::with_capacity(entities.len());
    let mut last: Option<Eid> = None;
    for e in entities {
        let id = e.id.ok_or("an entity without an id")?;
        if last.is_some_and(|l| l >= id) { return Err(format!("entities out of order at {}", id.text())); }
        last = Some(id);
        index.push((id, u32::try_from(w.b.len()).map_err(|_| "a chunk over 4 GB")?));
        put_entity(&mut w, &mut d, ix, e)?;
    }
    let dict_at = w.b.len() as u64;
    d.write(&mut w);
    let index_at = w.b.len() as u64;
    w.u(index.len() as u64);
    for (id, off) in &index { w.raw(&id.0.to_le_bytes()); w.raw(&off.to_le_bytes()); }
    let mut head = Vec::with_capacity(HEAD);
    head.extend_from_slice(MAGIC); head.extend_from_slice(&VERSION.to_le_bytes());
    head.extend_from_slice(&dict_at.to_le_bytes()); head.extend_from_slice(&index_at.to_le_bytes());
    w.b[..HEAD].copy_from_slice(&head);
    Ok(w.b)
}

pub struct Reader<'a> { b: &'a [u8], dict: Vec<String>, n: usize, index_body: usize }

impl<'a> Reader<'a> {
    pub fn open(b: &'a [u8]) -> Result<Self, String> {
        if b.len() < HEAD || &b[..8] != MAGIC { return Err("not a store chunk".into()); }
        if u32::from_le_bytes(b[8..12].try_into().unwrap()) != VERSION { return Err("chunk version".into()); }
        let mut r = R::at(b, 12);
        let (dict_at, index_at) = (r.u64le()? as usize, r.u64le()? as usize);
        let dict = Dict::read(&mut R::at(b, dict_at))?;
        let mut r = R::at(b, index_at);
        let n = r.u()? as usize;
        Ok(Reader { b, dict, n, index_body: r.i })
    }

    pub fn len(&self) -> usize { self.n }
    pub fn is_empty(&self) -> bool { self.n == 0 }

    pub fn slot(&self, k: usize) -> (Eid, usize) {
        let o = self.index_body + 12 * k;
        (Eid(u64::from_le_bytes(self.b[o..o + 8].try_into().unwrap())),
         u32::from_le_bytes(self.b[o + 8..o + 12].try_into().unwrap()) as usize)
    }

    /// One entity, by binary search on the index; the rest of the chunk is not read.
    pub fn get(&self, id: Eid, n: &Nodes) -> Result<Option<Entity>, String> {
        let (mut lo, mut hi) = (0usize, self.n);
        while lo < hi {
            let mid = (lo + hi) / 2;
            let (e, off) = self.slot(mid);
            match e.cmp(&id) {
                std::cmp::Ordering::Equal => return get_entity(&mut R::at(self.b, off), &self.dict, n, e).map(Some),
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
            }
        }
        Ok(None)
    }

    pub fn read_all(&self, n: &Nodes) -> Result<Vec<Entity>, String> {
        (0..self.n).map(|k| { let (e, off) = self.slot(k); get_entity(&mut R::at(self.b, off), &self.dict, n, e) }).collect()
    }
}
