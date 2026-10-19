//! Generates the chapter span index for the embedded corpora (see docs/adr/0001-corpus-index.md).
//! Same rules as tests/bundle_sources.py: one span per consecutive (book, chapter) run of rows;
//! four-field rows are source subsection headings and belong to no span.
use std::fmt::Write as _;
use std::{env, fs, path::Path};

fn main() {
    let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../src/data");
    let mut out = String::new();
    for (index, tool) in ["kjv", "grb", "vul"].into_iter().enumerate() {
        let path = data.join(format!("{tool}.tsv"));
        println!("cargo:rerun-if-changed={}", path.display());
        let text = fs::read_to_string(&path).expect("read embedded TSV");
        let mut spans: Vec<(&str, u16, usize, usize)> = Vec::new();
        let mut offset = 0;
        while offset < text.len() {
            let bytes = text.as_bytes();
            let mut end = offset;
            while end < bytes.len() && bytes[end] != b'\n' && bytes[end] != b'\r' {
                end += 1;
            }
            if end < bytes.len() {
                end += if bytes[end] == b'\r' && bytes.get(end + 1) == Some(&b'\n') {
                    2
                } else {
                    1
                };
            }
            let fields: Vec<&str> = text[offset..end]
                .trim_end_matches(['\r', '\n'])
                .splitn(6, '\t')
                .collect();
            if fields.len() != 4 {
                assert_eq!(
                    fields.len(),
                    6,
                    "{tool}: expected six fields at byte {offset}"
                );
                let chapter: u16 = fields[3].parse().expect("chapter number");
                match spans.last_mut() {
                    Some(last) if last.0 == fields[0] && last.1 == chapter => last.3 = end,
                    _ => spans.push((fields[0], chapter, offset, end)),
                }
            }
            offset = end;
        }
        writeln!(out, "pub static SPANS_{index}: [Span; {}] = [", spans.len()).unwrap();
        for (book, chapter, start, end) in spans {
            writeln!(
                out,
                "    Span {{ book: {book:?}, chapter: {chapter}, start: {start}, end: {end} }},"
            )
            .unwrap();
        }
        out.push_str("];\n");
    }
    fs::write(
        Path::new(&env::var("OUT_DIR").unwrap()).join("spans.rs"),
        out,
    )
    .unwrap();
    println!("cargo:rerun-if-changed=build.rs");
}
