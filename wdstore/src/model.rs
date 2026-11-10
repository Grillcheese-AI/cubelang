//! What the hot store keeps of an entity, exactly as the dump states it.
//!
//! No value is a float (order 0055): quantities, coordinates and their precisions stay the decimal text the dump
//! wrote; a time keeps its literal and, through its value node, its precision, time zone and calendar.

/// An entity id as Wikidata writes it: the kind letter and its number (Q42 = Item 42). Packed into a u64 so a
/// store row sorts by kind, then number; a lexeme's form or sense keeps its sub-number in the low 20 bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Eid(pub u64);

pub const KINDS: [char; 3] = ['Q', 'P', 'L'];
const SUB_BITS: u64 = 20;

impl Eid {
    /// "Q42", "P31", "L7", "L7-F2", "L7-S1" (forms and senses: sub kind 1 and 2).
    pub fn parse(s: &str) -> Option<Eid> {
        let k = KINDS.iter().position(|&c| s.starts_with(c))? as u64;
        let rest = &s[1..];
        let (num, sub) = match rest.split_once('-') {
            None => (rest, 0u64),
            Some((n, f)) => {
                let kind = match f.chars().next()? { 'F' => 1u64, 'S' => 2u64, _ => return None };
                let x: u64 = f[1..].parse().ok()?;
                if x >= 1 << (SUB_BITS - 2) { return None; }
                (n, (kind << (SUB_BITS - 2)) | x)
            }
        };
        if num.is_empty() || num.starts_with('0') || !num.bytes().all(|b| b.is_ascii_digit()) { return None; }
        let n: u64 = num.parse().ok()?;
        if n >= 1 << 36 { return None; }
        Some(Eid((k << 56) | (n << SUB_BITS) | sub))
    }

    pub fn kind(self) -> char { KINDS[(self.0 >> 56) as usize] }
    pub fn number(self) -> u64 { (self.0 >> SUB_BITS) & ((1 << 36) - 1) }

    pub fn text(self) -> String {
        let sub = self.0 & ((1 << SUB_BITS) - 1);
        let base = format!("{}{}", self.kind(), self.number());
        match sub >> (SUB_BITS - 2) {
            0 => base,
            k => format!("{}-{}{}", base, if k == 1 { 'F' } else { 'S' }, sub & ((1 << (SUB_BITS - 2)) - 1)),
        }
    }
}

/// A value node's or reference node's hash, as the dump names it (wdv: 32 hex digits, wdref: 40).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeHash(pub Vec<u8>);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Entity(Eid),
    /// A plain string: the string and external-id datatypes.
    Str(String),
    /// An IRI: url, Commons media, geo shapes, tabular data, a non-Wikidata entity.
    Iri(String),
    /// Monolingual text.
    Mono { text: String, lang: String },
    /// Any other typed literal, kept as written: a time (xsd:dateTime), a quantity (xsd:decimal), a coordinate
    /// (geo:wktLiteral), math. Its full value (precision, unit, calendar) is a value node, listed beside it.
    Typed { lit: String, dtype: String },
    SomeValue,
    NoValue,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rank { Deprecated = 0, Normal = 1, Preferred = 2 }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snak { pub pid: u32, pub value: Value }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Statement {
    /// The statement id after the entity's id (the part of `wds:Q42-...` past the first '-'), exactly as written.
    pub guid: String,
    pub pid: u32,
    pub rank: Rank,
    pub best: bool,
    pub value: Value,
    /// The main value's full value node (psv:), when it has one.
    pub value_node: Option<NodeHash>,
    /// Qualifiers in the store's hot set (the time ones: valid time), then the rest (cold): both kept, apart. A
    /// qualifier's value nodes (pqv:) are listed by property, as the dump states them: Turtle does not pair two
    /// values of one property with their nodes, so the pairing is made on reading, by the node's own value.
    pub hot_qualifiers: Vec<Snak>,
    pub hot_qualifier_nodes: Vec<(u32, NodeHash)>,
    pub cold_qualifiers: Vec<Snak>,
    pub cold_qualifier_nodes: Vec<(u32, NodeHash)>,
    pub references: Vec<NodeHash>,
}

/// A value node's fields, each the literal the dump wrote (`wikibase:timePrecision "11"^^xsd:integer` -> "11").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValueNode { pub hash: NodeHash, pub kind: String, pub fields: Vec<(String, String)> }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefNode { pub hash: NodeHash, pub snaks: Vec<Snak>, pub nodes: Vec<(u32, NodeHash)> }

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum NameKind { Label = 0, Alias = 1, Description = 2 }

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Name { pub lang: String, pub kind: NameKind, pub text: String }

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Entity {
    pub id: Option<Eid>,
    /// Recorded time, the base's side: the revision the dump holds and its timestamp, as written.
    pub revision: u64,
    pub modified: String,
    /// A property's datatype (`wikibase:propertyType`), e.g. "wikibase:Time".
    pub datatype: Option<String>,
    pub names: Vec<Name>,
    /// enwiki / frwiki article titles.
    pub sitelinks: Vec<(String, String)>,
    pub statements: Vec<Statement>,
}
