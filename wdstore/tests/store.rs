//! The store's claims, pinned: ids round-trip; the Turtle reader keeps every literal exactly; an entity's block
//! becomes exactly what the dump states (ranks, no value and some value, hot and cold qualifiers, references, the
//! revision, the projection of names and wikis, redirects); a chunk reads back to the same entities and a rebuild
//! writes the same bytes; no stored value is a float.

use wdstore::build;
use wdstore::codec::{guid_kind, R, W};
use wdstore::store::Store;
use wdstore::ingest::{block, Params};
use wdstore::model::*;
use wdstore::turtle::{triples, Term};

const ITEM: &str = r#"data:Q42 a schema:Dataset ;
	schema:about wd:Q42 ;
	schema:version "2448870000"^^xsd:integer ;
	schema:dateModified "2025-12-20T10:00:00Z"^^xsd:dateTime .

wd:Q42 a wikibase:Item ;
	rdfs:label "Douglas Adams"@en, "Douglas Adams"@fr, "Duglas Adams"@de ;
	skos:altLabel "D. \"N.\" Adams"@en ;
	schema:description "écrivain"@fr .

wd:Q42 p:P569 wds:Q42-AAAA-1 .
wds:Q42-AAAA-1 a wikibase:Statement, wikibase:BestRank ;
	wikibase:rank wikibase:NormalRank ;
	ps:P569 "1952-03-11T00:00:00Z"^^xsd:dateTime ;
	psv:P569 wdv:0123456789abcdef0123456789abcdef ;
	prov:wasDerivedFrom wdref:00000000000000000000000000000000000000aa .

wd:Q42 p:P1082 wds:Q42-bbbb-2 .
wds:Q42-bbbb-2 a wikibase:Statement ;
	wikibase:rank wikibase:DeprecatedRank ;
	ps:P1082 "+1234.50"^^xsd:decimal ;
	pq:P580 "2001-01-01T00:00:00Z"^^xsd:dateTime ;
	pqv:P580 wdv:ffffffffffffffffffffffffffffffff ;
	pq:P642 wd:Q515 ;
	pq:P642 _:b0 .

wd:Q42 p:P40 wds:Q42-cccc-3 .
wds:Q42-cccc-3 a wikibase:Statement, wdno:P40, wdno:P582 ;
	wikibase:rank wikibase:PreferredRank .

wdv:0123456789abcdef0123456789abcdef a wikibase:TimeValue ;
	wikibase:timeValue "1952-03-11T00:00:00Z"^^xsd:dateTime ;
	wikibase:timePrecision "11"^^xsd:integer ;
	wikibase:timeCalendarModel <http://www.wikidata.org/entity/Q1985727> .

wdref:00000000000000000000000000000000000000aa a wikibase:Reference ;
	pr:P248 wd:Q5375741 .

<https://en.wikipedia.org/wiki/Douglas_Adams> a schema:Article ;
	schema:about wd:Q42 ;
	schema:isPartOf <https://en.wikipedia.org/> ;
	schema:name "Douglas Adams"@en .

<https://de.wikipedia.org/wiki/Douglas_Adams> a schema:Article ;
	schema:about wd:Q42 ;
	schema:isPartOf <https://de.wikipedia.org/> ;
	schema:name "Douglas Adams"@de .
"#;

const HUMANS: &str = r#"data:Q45 a schema:Dataset ;
	schema:about wd:Q45 ;
	schema:version "3"^^xsd:integer .

wd:Q45 a wikibase:Item ;
	p:P31 wds:Q45-1 .
wds:Q45-1 a wikibase:Statement ;
	wikibase:rank wikibase:DeprecatedRank ;
	ps:P31 wd:Q5 .

data:Q44 a schema:Dataset ;
	schema:about wd:Q44 ;
	schema:version "4"^^xsd:integer .

wd:Q44 a wikibase:Item ;
	p:P31 wds:Q44-1 .
wds:Q44-1 a wikibase:Statement, wikibase:BestRank ;
	wikibase:rank wikibase:NormalRank ;
	ps:P31 wd:Q5 .
"#;

const REDIRECT: &str = "data:Q7 a schema:Dataset ;\n\tschema:about wd:Q7 ;\n\tschema:version \"5\"^^xsd:integer .\n\nwd:Q7 owl:sameAs wd:Q42 .\n";

fn q(n: &str) -> Eid { Eid::parse(n).unwrap() }

#[test]
fn ids_round_trip_and_reject_what_is_not_an_id() {
    for s in ["Q1", "Q42", "Q137645055", "P31", "L7", "L7-F2", "L7-S1"] { assert_eq!(q(s).text(), s); }
    for s in ["Q0", "Q01", "X1", "Q", "Q1-X2", "Q-1"] { assert!(Eid::parse(s).is_none(), "{s}"); }
    assert!(q("P1") > q("Q999999999") && q("Q2") > q("Q1"));
}

#[test]
fn the_reader_keeps_literals_exactly() {
    let t = triples("wd:Q1 rdfs:label \"a \\\"b\\\" \\u00e9\\n\"@en-gb , \"x\" ; wikibase:geoGlobe <http://www.wikidata.org/entity/Q2> ;\n\tschema:x \"1.0\"^^<http://example.org/t> ; .").unwrap();
    assert_eq!(t.len(), 4);
    assert_eq!(t[0].2, Term::Lit { lex: "a \"b\" é\n".into(), lang: Some("en-gb".into()), dtype: None });
    assert_eq!(t[1].2, Term::Lit { lex: "x".into(), lang: None, dtype: None });
    assert_eq!(t[2].2, Term::Name("wd:Q2".into()));
    assert_eq!(t[3].2, Term::Lit { lex: "1.0".into(), lang: None, dtype: Some("http://example.org/t".into()) });
}

#[test]
fn a_block_becomes_what_the_dump_states() {
    let b = block(ITEM, &Params::default()).unwrap();
    let e = b.entity.unwrap();
    assert_eq!((e.id, e.revision, e.modified.as_str()), (Some(q("Q42")), 2448870000, "2025-12-20T10:00:00Z"));
    let names: Vec<_> = e.names.iter().map(|n| (n.lang.as_str(), n.kind, n.text.as_str())).collect();
    assert_eq!(names, vec![("en", NameKind::Label, "Douglas Adams"), ("en", NameKind::Alias, "D. \"N.\" Adams"),
                           ("fr", NameKind::Label, "Douglas Adams"), ("fr", NameKind::Description, "écrivain")]);
    assert_eq!(e.sitelinks, vec![("https://en.wikipedia.org/".to_string(), "Douglas Adams".to_string())]);
    let [father, born, pop] = &e.statements[..] else { panic!("{:?}", e.statements) };
    assert_eq!((born.pid, born.rank, born.best, born.guid.as_str()), (569, Rank::Normal, true, "AAAA-1"));
    assert_eq!(born.value, Value::Typed { lit: "1952-03-11T00:00:00Z".into(), dtype: "xsd:dateTime".into() });
    assert_eq!(born.value_node.as_ref().unwrap().0, b"0123456789abcdef0123456789abcdef");
    assert_eq!(born.references.len(), 1);
    assert_eq!((pop.rank, &pop.value), (Rank::Deprecated, &Value::Typed { lit: "+1234.50".into(), dtype: "xsd:decimal".into() }));
    assert_eq!(pop.hot_qualifiers, vec![Snak { pid: 580, value: Value::Typed { lit: "2001-01-01T00:00:00Z".into(), dtype: "xsd:dateTime".into() } }]);
    assert_eq!(pop.hot_qualifier_nodes.len(), 1);
    assert_eq!(pop.cold_qualifiers, vec![Snak { pid: 642, value: Value::Entity(q("Q515")) }, Snak { pid: 642, value: Value::SomeValue }]);
    assert_eq!((father.pid, father.rank, &father.value), (40, Rank::Preferred, &Value::NoValue));
    assert_eq!(father.hot_qualifiers, vec![Snak { pid: 582, value: Value::NoValue }]);   // a qualifier's no value
    assert_eq!(b.value_nodes[0].fields.len(), 3);
    assert_eq!(b.ref_nodes[0].snaks, vec![Snak { pid: 248, value: Value::Entity(q("Q5375741")) }]);
    let r = block(REDIRECT, &Params::default()).unwrap();
    assert!(r.entity.is_none());
    assert_eq!(r.redirects, vec![(q("Q7"), q("Q42"))]);
}

#[test]
fn a_statement_with_no_main_snak_is_an_error_not_a_no_value() {
    let bad = "data:Q1 a schema:Dataset ;\n\tschema:about wd:Q1 ;\n\tschema:version \"1\"^^xsd:integer .\n\nwd:Q1 a wikibase:Item ;\n\tp:P31 wds:Q1-x .\n\nwds:Q1-x a wikibase:Statement ;\n\twikibase:rank wikibase:NormalRank .\n";
    assert!(block(bad, &Params::default()).unwrap_err().contains("no main snak"));
}

fn built(name: &str) -> (String, Store) { built_with(name, &format!("{ITEM}\n{}\n{HUMANS}\n{REDIRECT}", ITEM.replace("Q42", "Q43")), vec![]) }

fn built_with(name: &str, text: &str, cold: Vec<Eid>) -> (String, Store) {
    let dir = std::env::temp_dir().join(format!("wdstore_test_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("in.ttl");
    std::fs::write(&input, text).unwrap();
    let out = dir.join("store").to_str().unwrap().to_string();
    let scratch = dir.join("scratch").to_str().unwrap().to_string();
    build::build(&[input.to_str().unwrap().to_string()], &out, &scratch, 1, cold, 2, 16, None).unwrap();
    let s = Store::open(&out).unwrap();
    (out, s)
}

#[test]
fn a_built_store_reads_back_what_was_ingested_and_a_rebuild_writes_the_same_bytes() {
    let (out, st) = built("a");
    let want = block(ITEM, &Params::default()).unwrap().entity.unwrap();
    assert_eq!(st.get(q("Q42")).unwrap().unwrap(), want);
    assert_eq!(st.get(q("Q43")).unwrap().unwrap().statements, want.statements);   // the same block under Q43
    assert!(st.get(q("Q46")).unwrap().is_none() && st.get(q("Q1")).unwrap().is_none());
    assert_eq!(st.resolve(q("Q7")).unwrap(), q("Q42"));
    assert_eq!(st.resolve(q("Q42")).unwrap(), q("Q42"));
    assert!(st.check().unwrap().contains("4 hot and 0 cold entities"));
    let hits = st.who(31, q("Q5")).unwrap();
    assert_eq!(hits.iter().map(|h| (h.subject, h.rank, h.best)).collect::<Vec<_>>(),
               vec![(q("Q44"), Rank::Normal, true), (q("Q45"), Rank::Deprecated, false)]);
    assert!(st.who(31, q("Q6")).unwrap().is_empty() && st.who(279, q("Q5")).unwrap().is_empty());
    let v = &st.nodes.values;
    let node = (0..v.len() as u32).find_map(|i| v.value(i).unwrap()).unwrap();
    assert_eq!(node.fields.iter().find(|f| f.0 == "wikibase:timePrecision").unwrap().1, "11");
    let (out2, _) = built("b");
    for f in ["chunk_hot_00000.wds", "chunk_hot_00003.wds", "ids.wds", "values.wds", "refs.wds", "redirects.wds", "reverse.wds",
              "names_exact.fst", "names_folded.fst", "names.post"] {
        assert_eq!(std::fs::read(format!("{out}/{f}")).unwrap(), std::fs::read(format!("{out2}/{f}")).unwrap(), "{f}");
    }
}

#[test]
fn statement_ids_round_trip_in_every_case() {
    for g in ["f89a5f74-ab80-4093-976c-993a927bdcb5", "F89A5F74-AB80-4093-976C-993A927BDCB5",
              "F89a5f74-AB80-4093-976C-993A927BDCB5", "12345678-1234-1234-1234-123456789012", "AAAA-1", ""] {
        let k = guid_kind(g);
        let mut w = W::default();
        w.guid(g, k);
        assert_eq!(R::new(&w.b).guid(k).unwrap(), g);
        assert_eq!(w.b.len(), if k == 0 { g.len() + 1 } else { 16 });
    }
}

#[test]
fn no_stored_value_is_a_float() {
    for src in [include_str!("../src/model.rs"), include_str!("../src/codec.rs"), include_str!("../src/chunk.rs"),
                include_str!("../src/ingest.rs"), include_str!("../src/turtle.rs"), include_str!("../src/nodes.rs"),
                include_str!("../src/reverse.rs"), include_str!("../src/names.rs"), include_str!("../src/store.rs"), include_str!("../src/spill.rs"),
                include_str!("../src/bz2par.rs"), include_str!("../src/writer.rs"), include_str!("../src/resume.rs"), include_str!("../src/build.rs")] {
        for bad in ["f32", "f64", "parse::<f"] { assert!(!src.contains(bad), "a float type in the store's code: {bad}"); }
    }
}

#[test]
fn a_node_written_in_two_places_is_read_as_one_and_cold_classes_go_cold() {
    let split = r#"data:Q50 a schema:Dataset ;
	schema:about wd:Q50 ;
	schema:version "1"^^xsd:integer .

wd:Q50 a wikibase:Item ;
	p:P2048 wds:Q50-a .

wds:Q50-a a wikibase:Statement ;
	wikibase:rank wikibase:NormalRank ;
	ps:P2048 "+87.7"^^xsd:decimal ;
	psv:P2048 wdv:aa .

wdv:aa a wikibase:QuantityValue ;
	wikibase:quantityAmount "+87.7"^^xsd:decimal .

data:Q51 a schema:Dataset ;
	schema:about wd:Q51 ;
	schema:version "2"^^xsd:integer .

wd:Q51 a wikibase:Item .

wdv:aa wikibase:quantityNormalized wdv:bb .
"#;
    let (_, st) = built_with("split", &format!("{split}\n{HUMANS}"), vec![q("Q5")]);
    let v = &st.nodes.values;
    let aa = (0..v.len() as u32).find(|&i| v.hash(i).unwrap().0 == b"aa").unwrap();
    let n = v.value(aa).unwrap().unwrap();
    assert_eq!((n.kind.as_str(), n.fields.len()), ("wikibase:QuantityValue", 2));
    assert_eq!(v.undefined().unwrap(), 0);     // wdv:bb is a field's text here (normalized values are derived), not a node
    assert!(st.check().unwrap().contains("2 hot and 2 cold entities"));
    assert!(st.locate(q("Q44")).unwrap().unwrap().0 && !st.locate(q("Q50")).unwrap().unwrap().0);
    assert!(st.who(31, q("Q5")).unwrap().is_empty());                         // the indexes are the hot set's
}

#[test]
fn a_name_finds_every_entity_it_may_mean_in_id_order_exact_then_folded_then_typo() {
    use wdstore::names::{folded_key, Found, How};
    assert_eq!(folded_key("  École   Normale-Supérieure "), "ecole normale superieure");
    let (_, st) = built("n");
    let ents = |f: Found| match f { Found::Hits(h) => h.iter().map(|x| (x.entity.text(), x.lang.clone(), x.how)).collect::<Vec<_>>(), x => panic!("{x:?}") };
    let ex = ents(st.name("douglas   ADAMS").unwrap());
    assert_eq!(ex, vec![("Q42".into(), "en".into(), How::Exact), ("Q42".into(), "fr".into(), How::Exact),
                        ("Q43".into(), "en".into(), How::Exact), ("Q43".into(), "fr".into(), How::Exact)]);
    assert!(ents(st.name("Douglás Adams").unwrap()).iter().all(|h| h.2 == How::Folded));
    assert_eq!(ents(st.name("D N Adams").unwrap()).len(), 2);                 // the alias, folded, on both
    assert!(ents(st.name("Douglas Adans").unwrap()).iter().all(|h| h.2 == How::Typo(1)));
    assert_eq!(st.name("Adams").unwrap(), Found::None);                       // a part of a name is not the name
    assert_eq!(st.name("Dougl").unwrap(), Found::None);
}

#[test]
fn one_changed_byte_fails_the_check() {
    for f in ["chunk_hot_00001.wds", "reverse.wds", "names.post", "values.wds"] {
        let (out, st) = built(&format!("tamper_{}", f.replace('.', "_")));
        drop(st);
        let p = format!("{out}/{f}");
        let mut b = std::fs::read(&p).unwrap();
        let at = b.len() / 2;
        b[at] ^= 1;
        std::fs::write(&p, &b).unwrap();
        let r = Store::open(&out).and_then(|s| s.check());
        assert!(r.is_err(), "{f}: a changed byte passed the check");
    }
}
