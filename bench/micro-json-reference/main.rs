//! Std JSON pair: construct a document, then parse/stringify/drop ten times.
use std::fmt::Write;

fn document(n: u64) -> String {
    let mut out = String::from("[");
    for i in 0..n {
        if i != 0 {
            out.push(',');
        }
        write!(out, "{{\"id\":{i},\"name\":\"user{i}\",\"score\":{}.5,\"tags\":[\"a\",\"bb\",\"ccc\"],\"active\":true,\"ref\":null}}", i * 7 % 1000).unwrap();
    }
    out.push(']');
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
            let value: serde_json::Value = serde_json::from_str(&doc).unwrap();
            let output = serde_json::to_string(&value).unwrap();
            sum = sum.wrapping_add(output.len() as u64);
        }
    }
    println!("{sum}");
}
