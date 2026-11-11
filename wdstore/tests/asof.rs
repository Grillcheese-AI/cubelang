//! Valid time against an asked date: day numbers in both calendars, spans by precision, and holds / does not /
//! too coarse / undated.

use wdstore::asof::{day_number, holds, span, Holds};
use wdstore::model::*;

fn node(h: &str, lit: &str, prec: u8, cal: &str) -> ValueNode {
    ValueNode { hash: NodeHash(h.as_bytes().to_vec()), kind: "wikibase:TimeValue".into(), fields: vec![
        ("wikibase:timeCalendarModel".into(), format!("http://www.wikidata.org/entity/{cal}")),
        ("wikibase:timePrecision".into(), prec.to_string()), ("wikibase:timeValue".into(), lit.into())] }
}

fn st(quals: &[(u32, &str, &str)]) -> Statement {
    Statement { guid: "g".into(), pid: 169, rank: Rank::Normal, best: true, value: Value::Entity(Eid::parse("Q1").unwrap()),
        value_node: None, cold_qualifiers: vec![], cold_qualifier_nodes: vec![], references: vec![],
        hot_qualifiers: quals.iter().map(|(p, lit, _)| Snak { pid: *p, value: Value::Typed { lit: lit.to_string(), dtype: "xsd:dateTime".into() } }).collect(),
        hot_qualifier_nodes: quals.iter().map(|(p, _, h)| (*p, NodeHash(h.as_bytes().to_vec()))).collect() }
}

fn year(y: i64) -> (i64, i64) { span(&format!("{y}-01-01T00:00:00Z"), 9, "Q1985727").unwrap() }

#[test]
fn day_numbers_and_spans() {
    assert_eq!(day_number(2000, 1, 1, false), 2451545);
    assert_eq!(day_number(1582, 10, 4, true) + 1, day_number(1582, 10, 15, false));   // the calendar change
    assert_eq!(span("1952-03-11T00:00:00Z", 11, "Q1985727"), Some((2434083, 2434083)));
    let feb = span("2024-02-00T00:00:00Z", 10, "Q1985727").unwrap();
    assert_eq!(feb.1 - feb.0 + 1, 29);
    assert_eq!(span("1955-00-00T00:00:00Z", 8, "Q1985727"), Some((year(1950).0, year(1959).1)));       // the 1950s
    assert_eq!(span("2000-00-00T00:00:00Z", 7, "Q1985727"), Some((year(1901).0, year(2000).1)));       // 20th century
    assert_eq!(span("1952-03-11T00:00:00Z", 11, "Q123"), None);                                         // unknown calendar
}

#[test]
fn a_statement_holds_does_not_or_the_dates_are_too_coarse() {
    let nodes = [node("s", "2010-01-01T00:00:00Z", 9, "Q1985727"), node("e", "2019-06-30T00:00:00Z", 11, "Q1985727"),
                 node("p", "2020-01-01T00:00:00Z", 9, "Q1985727")];
    let find = |h: &NodeHash| nodes.iter().find(|n| &n.hash == h).cloned();
    let ceo = st(&[(580, "2010-01-01T00:00:00Z", "s"), (582, "2019-06-30T00:00:00Z", "e")]);
    assert_eq!(holds(&ceo, year(2015), &find), Holds::Holds);
    assert_eq!(holds(&ceo, year(2010), &find), Holds::TooCoarse);     // started some time in 2010: all of 2010 cannot be said
    assert_eq!(holds(&ceo, year(2019), &find), Holds::TooCoarse);     // ended mid-2019: "in 2019" cannot say
    assert_eq!(holds(&ceo, year(2021), &find), Holds::DoesNot);
    assert_eq!(holds(&ceo, span("2019-03-01T00:00:00Z", 11, "Q1985727").unwrap(), &find), Holds::Holds);
    assert_eq!(holds(&st(&[]), year(2015), &find), Holds::Undated);
    let pop = st(&[(585, "2020-01-01T00:00:00Z", "p")]);
    assert_eq!(holds(&pop, year(2020), &find), Holds::Holds);
    assert_eq!(holds(&pop, span("2021-05-01T00:00:00Z", 11, "Q1985727").unwrap(), &find), Holds::DoesNot);
    let open = st(&[(580, "2010-01-01T00:00:00Z", "s")]);
    assert_eq!(holds(&open, year(2030), &find), Holds::Holds);
    assert_eq!(holds(&open, year(2005), &find), Holds::DoesNot);
    let unknown_node = st(&[(580, "2010-01-01T00:00:00Z", "zz")]);
    assert_eq!(holds(&unknown_node, year(2015), &find), Holds::TooCoarse);   // a start whose precision is unknown
}
