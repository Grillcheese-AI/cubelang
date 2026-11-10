//! The Turtle the Wikidata dump writes, read token by token (not line by line): IRIs, prefixed names, literals
//! with their escapes, language tags and datatypes, blank nodes, `a`, and `, ; .`. Full IRIs in Wikidata's own
//! namespaces are written back as the dump's prefixed names (`<http://www.wikidata.org/entity/Q2>` -> `wd:Q2`), so
//! one value has one spelling.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Term {
    /// A prefixed name, as `prefix:local`.
    Name(String),
    Iri(String),
    Lit { lex: String, lang: Option<String>, dtype: Option<String> },
    Blank(String),
}

pub type Triple = (Term, Term, Term);

const SPACES: [(&str, &str); 22] = [
    ("http://www.wikidata.org/entity/statement/", "wds:"), ("http://www.wikidata.org/value/", "wdv:"),
    ("http://www.wikidata.org/reference/", "wdref:"), ("http://www.wikidata.org/prop/direct/", "wdt:"),
    ("http://www.wikidata.org/prop/direct-normalized/", "wdtn:"), ("http://www.wikidata.org/prop/statement/value/", "psv:"),
    ("http://www.wikidata.org/prop/statement/value-normalized/", "psn:"), ("http://www.wikidata.org/prop/statement/", "ps:"),
    ("http://www.wikidata.org/prop/qualifier/value/", "pqv:"), ("http://www.wikidata.org/prop/qualifier/value-normalized/", "pqn:"),
    ("http://www.wikidata.org/prop/qualifier/", "pq:"), ("http://www.wikidata.org/prop/reference/value/", "prv:"),
    ("http://www.wikidata.org/prop/reference/value-normalized/", "prn:"), ("http://www.wikidata.org/prop/reference/", "pr:"),
    ("http://www.wikidata.org/prop/novalue/", "wdno:"), ("http://www.wikidata.org/prop/", "p:"),
    ("http://www.wikidata.org/entity/", "wd:"), ("http://www.wikidata.org/wiki/Special:EntityData/", "data:"),
    ("http://wikiba.se/ontology#", "wikibase:"), ("http://schema.org/", "schema:"),
    ("http://www.w3.org/2001/XMLSchema#", "xsd:"), ("http://www.w3.org/2000/01/rdf-schema#", "rdfs:"),
];

fn shorten(iri: String) -> Term {
    for (ns, p) in SPACES {
        if let Some(local) = iri.strip_prefix(ns) {
            if !local.is_empty() && local.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
                return Term::Name(format!("{p}{local}"));
            }
        }
    }
    Term::Iri(iri)
}

#[derive(Debug, PartialEq)]
pub enum Tok { T(Term), A, Comma, Semi, Dot, Prefix }

pub struct Lexer<'a> { s: &'a [u8], i: usize }

impl<'a> Lexer<'a> {
    pub fn new(s: &'a str) -> Self { Lexer { s: s.as_bytes(), i: 0 } }

    fn err(&self, what: &str) -> String {
        let a = self.i.saturating_sub(40);
        format!("{what} at byte {}: {:?}", self.i, String::from_utf8_lossy(&self.s[a..(self.i + 40).min(self.s.len())]))
    }

    fn word(&mut self) -> &'a str {
        let st = self.i;
        while self.i < self.s.len() && !matches!(self.s[self.i], b' ' | b'\t' | b'\n' | b'\r' | b',' | b';') {
            self.i += 1;
        }
        // a '.' ends the statement when it ends the word
        while self.i > st + 1 && self.s[self.i - 1] == b'.' { self.i -= 1; }
        std::str::from_utf8(&self.s[st..self.i]).unwrap()
    }

    fn literal(&mut self) -> Result<String, String> {
        let long = self.s[self.i..].starts_with(b"\"\"\"");
        self.i += if long { 3 } else { 1 };
        let mut out: Vec<u8> = Vec::new();
        loop {
            let Some(&c) = self.s.get(self.i) else { return Err(self.err("unclosed literal")) };
            if c == b'"' && (!long || self.s[self.i..].starts_with(b"\"\"\"")) {
                self.i += if long { 3 } else { 1 };
                return String::from_utf8(out).map_err(|_| self.err("literal not UTF-8"));
            }
            if c != b'\\' { out.push(c); self.i += 1; continue; }
            let e = *self.s.get(self.i + 1).ok_or_else(|| self.err("bad escape"))?;
            self.i += 2;
            let simple = match e { b't' => Some('\t'), b'n' => Some('\n'), b'r' => Some('\r'), b'b' => Some('\u{8}'),
                b'f' => Some('\u{c}'), b'"' => Some('"'), b'\'' => Some('\''), b'\\' => Some('\\'), _ => None };
            let ch = match (simple, e) {
                (Some(ch), _) => ch,
                (None, b'u') | (None, b'U') => {
                    let n = if e == b'u' { 4 } else { 8 };
                    let hex = std::str::from_utf8(self.s.get(self.i..self.i + n).ok_or_else(|| self.err("bad \\u"))?)
                        .map_err(|_| self.err("bad \\u"))?;
                    self.i += n;
                    char::from_u32(u32::from_str_radix(hex, 16).map_err(|_| self.err("bad \\u"))?)
                        .ok_or_else(|| self.err("bad code point"))?
                }
                _ => return Err(self.err("unknown escape")),
            };
            let mut b = [0u8; 4];
            out.extend_from_slice(ch.encode_utf8(&mut b).as_bytes());
        }
    }

    pub fn next_tok(&mut self) -> Result<Option<Tok>, String> {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() { self.i += 1; }
        let Some(&c) = self.s.get(self.i) else { return Ok(None) };
        Ok(Some(match c {
            b',' => { self.i += 1; Tok::Comma }
            b';' => { self.i += 1; Tok::Semi }
            b'.' => { self.i += 1; Tok::Dot }
            b'<' => {
                let end = self.s[self.i..].iter().position(|&b| b == b'>').ok_or_else(|| self.err("unclosed IRI"))?;
                let iri = String::from_utf8(self.s[self.i + 1..self.i + end].to_vec()).map_err(|_| self.err("IRI"))?;
                self.i += end + 1;
                Tok::T(shorten(iri))
            }
            b'"' => {
                let lex = self.literal()?;
                let (mut lang, mut dtype) = (None, None);
                if self.s.get(self.i) == Some(&b'@') {
                    self.i += 1;
                    lang = Some(self.word().to_string());
                } else if self.s[self.i..].starts_with(b"^^") {
                    self.i += 2;
                    dtype = Some(match self.next_tok()? { Some(Tok::T(Term::Name(n))) => n,
                        Some(Tok::T(Term::Iri(i))) => i, _ => return Err(self.err("bad datatype")) });
                }
                Tok::T(Term::Lit { lex, lang, dtype })
            }
            b'@' => { let w = self.word(); if w == "@prefix" { Tok::Prefix } else { return Err(self.err("directive")) } }
            _ => {
                let w = self.word();
                if w == "a" { Tok::A } else if let Some(b) = w.strip_prefix("_:") { Tok::T(Term::Blank(b.to_string())) }
                else if w.contains(':') { Tok::T(Term::Name(w.to_string())) }
                else { Tok::T(Term::Lit { lex: w.to_string(), lang: None, dtype: Some("bare".into()) }) }
            }
        }))
    }
}

/// Every triple of a text holding whole Turtle statements (`@prefix` lines skipped).
pub fn triples(text: &str) -> Result<Vec<Triple>, String> {
    let mut lx = Lexer::new(text);
    let mut out = Vec::new();
    let rdf_type = Term::Name("rdf:type".into());
    loop {
        let subj = match lx.next_tok()? {
            None => return Ok(out),
            Some(Tok::Prefix) => { while !matches!(lx.next_tok()?, Some(Tok::Dot) | None) {} continue; }
            Some(Tok::T(t)) => t,
            Some(t) => return Err(lx.err(&format!("subject expected, got {t:?}"))),
        };
        'preds: loop {
            let pred = match lx.next_tok()? { Some(Tok::A) => rdf_type.clone(), Some(Tok::T(t)) => t,
                Some(Tok::Dot) => break 'preds, t => return Err(lx.err(&format!("predicate expected, got {t:?}"))) };
            loop {
                let obj = match lx.next_tok()? { Some(Tok::T(t)) => t, t => return Err(lx.err(&format!("object, got {t:?}"))) };
                out.push((subj.clone(), pred.clone(), obj));
                match lx.next_tok()? {
                    Some(Tok::Comma) => continue,
                    Some(Tok::Semi) => {
                        // a ';' may come right before the '.'
                        break;
                    }
                    Some(Tok::Dot) => break 'preds,
                    t => return Err(lx.err(&format!("',', ';' or '.' expected, got {t:?}"))),
                }
            }
        }
    }
}
