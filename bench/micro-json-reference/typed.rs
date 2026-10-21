//! Typed JSON decoding pair for bench/micro/programs/extended/json_typed*.hir.
use std::fmt::Write;

#[derive(serde::Deserialize)]
struct Row {
    id: u64,
    name: String,
    #[allow(dead_code)]
    score: f64,
    tags: Vec<String>,
    #[allow(dead_code)]
    active: bool,
    #[allow(dead_code)]
    reference: Option<String>,
}

#[derive(serde::Deserialize)]
struct Document {
    rows: Vec<Row>,
}

fn document(n: u64) -> String {
    let mut out = String::from("{\"rows\":[");
    for i in 0..n {
        if i != 0 {
            out.push(',');
        }
        write!(out, "{{\"id\":{i},\"name\":\"user{i}\",\"score\":{}.5,\"tags\":[\"a\",\"bb\",\"ccc\"],\"active\":true,\"reference\":null}}", i * 7 % 1000).unwrap();
    }
    out.push_str("]}");
    out
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n: u64 = args[1].parse().unwrap();
    let epochs: u64 = args[2].parse().unwrap();
    let mut sum = 0u64;
    for salt in 0..epochs {
        let doc = document(n + salt);
        for _ in 0..10 {
            let value: Document = serde_json::from_str(&doc).unwrap();
            for row in value.rows {
                sum = sum.wrapping_add(row.id + row.name.len() as u64 + row.tags.len() as u64);
            }
        }
    }
    println!("{sum}");
}
