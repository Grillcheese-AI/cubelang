//! A build from bzip2 parts, stopped after a checkpoint and run again, writes the same store, byte for byte, as a
//! build never stopped; a checkpoint for another build is set aside and the build starts over.

use std::io::Write;

fn entity(i: u64) -> String {
    let cls = if i % 3 == 0 { "Q13442814" } else { "Q5" };
    format!(concat!("data:Q{i} a schema:Dataset ;\n\tschema:about wd:Q{i} ;\n\tschema:version \"{i}\"^^xsd:integer .\n\n",
        "wd:Q{i} a wikibase:Item ;\n\trdfs:label \"Item {i} \u{e9}t\u{e9}\"@en ;\n\tp:P31 wds:Q{i}-a ;\n\tp:P569 wds:Q{i}-b .\n\n",
        "wds:Q{i}-a a wikibase:Statement ;\n\twikibase:rank wikibase:NormalRank ;\n\tps:P31 wd:{cls} .\n\n",
        "wds:Q{i}-b a wikibase:Statement ;\n\twikibase:rank wikibase:NormalRank ;\n\tps:P569 \"19{y:02}-01-01T00:00:00Z\"^^xsd:dateTime ;\n",
        "\tpsv:P569 wdv:{h:032x} ;\n\tprov:wasDerivedFrom wdref:{r:040x} .\n\n",
        "wdv:{h:032x} a wikibase:TimeValue ;\n\twikibase:timeValue \"19{y:02}-01-01T00:00:00Z\"^^xsd:dateTime ;\n\twikibase:timePrecision \"9\"^^xsd:integer .\n\n",
        "wdref:{r:040x} a wikibase:Reference ;\n\tpr:P248 wd:Q{src} .\n\n{redir}"),
        i = i, cls = cls, y = i % 100, h = i % 97, r = i % 13, src = 1000 + i % 13,
        redir = if i % 50 == 0 { format!("data:Q{} a schema:Dataset ;\n\tschema:about wd:Q{} ;\n\tschema:version \"1\"^^xsd:integer .\n\nwd:Q{} owl:sameAs wd:Q{i} .\n\n", 900000 + i, 900000 + i, 900000 + i) } else { String::new() })
}

fn parts(dir: &std::path::Path) -> Vec<String> {
    let text: String = (1..=3000).map(entity).collect();
    let mut e = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::new(1));
    e.write_all(text.as_bytes()).unwrap();
    let file = e.finish().unwrap();
    std::fs::create_dir_all(dir).unwrap();
    file.chunks(40_000).enumerate().map(|(k, c)| {
        let p = dir.join(format!("part_{k:05}"));
        std::fs::write(&p, c).unwrap();
        p.to_str().unwrap().to_string()
    }).collect()
}

fn files(dir: &str) -> Vec<(String, Vec<u8>)> {
    let mut v: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir).unwrap().filter_map(|e| {
        let e = e.unwrap();
        let n = e.file_name().to_str().unwrap().to_string();
        (n.ends_with(".wds") || n.ends_with(".fst") || n.ends_with(".post")).then(|| (n, std::fs::read(e.path()).unwrap()))
    }).collect();
    v.sort();
    v
}

#[test]
fn a_stopped_build_resumed_writes_the_same_store_as_one_never_stopped() {
    let root = std::env::temp_dir().join(format!("wdstore_resume_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let ps = parts(&root.join("parts"));
    let d = |n: &str| root.join(n).to_str().unwrap().to_string();
    let cold = vec![wdstore::model::Eid::parse("Q13442814").unwrap()];
    let run = |out: &str, stop: Option<u64>| wdstore::build::build_batched(&ps, out, &format!("{out}_scratch"), 200, cold.clone(), 3, 2, stop, 60_000);
    let whole = run(&d("whole"), None).unwrap();
    assert!(whole.contains("\"failed\": 0") && whole.contains("\"redirects\": 60"), "{whole}");
    let first = run(&d("resumed"), Some(4)).unwrap();
    assert!(first.starts_with("stopped after 4 batches"));
    let again = run(&d("resumed"), None).unwrap();
    assert!(again.contains("\"resumed_at_batch\": 4"), "{again}");
    let (a, b) = (files(&d("whole")), files(&d("resumed")));
    assert_eq!(a.iter().map(|x| &x.0).collect::<Vec<_>>(), b.iter().map(|x| &x.0).collect::<Vec<_>>());
    for (x, y) in a.iter().zip(&b) { assert!(x.1 == y.1, "{} differs after resuming", x.0); }
    let st = wdstore::store::Store::open(&d("resumed")).unwrap();
    assert!(st.check().unwrap().contains("2000 hot and 1000 cold entities"));
    // a checkpoint for other parameters is set aside: the build starts over and still gives the same store
    let _ = run(&d("other"), Some(2)).unwrap();
    let redo = wdstore::build::build_batched(&ps, &d("other"), &format!("{}_scratch", d("other")), 200, cold.clone(), 3, 3, None, 60_000).unwrap();
    assert!(redo.contains("\"resumed_at_batch\": null"));
    assert!(std::path::Path::new(&format!("{}/checkpoint.stale.txt", d("other"))).exists());
}
