// Convert CSV (a file, stdin or a built-in sample) to a JSON array of objects, one per
// row keyed by the header; optionally decode, re-encode and SHA-256 one base64 column.
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, BufReader, Read, Stdin};

#[derive(Debug)]
pub enum Error {
    Flags(ParseError),
    Csv(csv::Error),
    Input(io::Error),
    Files(io::Error),
    Open(io::Error),
    Output(io::Error),
    Encoding(base64::DecodeError, String),
    Invalid,
}

#[derive(Debug)]
pub enum ParseError {
    UnknownFlag(usize),
    MissingValue(usize),
    UnexpectedValue(usize),
}

struct FileSource {
    buffer: BufReader<File>,
}

struct InputSource {
    buffer: BufReader<Stdin>,
}

struct MemorySource {
    data: Vec<u8>,
    offset: usize,
}

fn file_refill(source: &mut FileSource, out: &mut Vec<u8>, max: usize) -> Result<usize, Error> {
    let mut chunk = vec![0; max];
    let n = source.buffer.read(&mut chunk).map_err(Error::Files)?;
    out.extend_from_slice(&chunk[..n]);
    Ok(n)
}

fn input_refill(source: &mut InputSource, out: &mut Vec<u8>, max: usize) -> Result<usize, Error> {
    let mut chunk = vec![0; max];
    let n = source.buffer.read(&mut chunk).map_err(Error::Input)?;
    out.extend_from_slice(&chunk[..n]);
    Ok(n)
}

fn memory_refill(source: &mut MemorySource, out: &mut Vec<u8>, max: usize) -> Result<usize, Error> {
    let left = source.data.len() - source.offset;
    let count = if left < max { left } else { max };
    for i in 0..count {
        out.push(source.data[source.offset + i]);
    }
    source.offset += count;
    Ok(count)
}

// A byte stream over any source and its refill function, for the CSV reader.
struct Refilled<'a, S> {
    source: &'a mut S,
    refill: fn(&mut S, &mut Vec<u8>, usize) -> Result<usize, Error>,
    failure: Option<Error>,
}

impl<S> Read for Refilled<'_, S> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut out = Vec::with_capacity(buf.len());
        match (self.refill)(self.source, &mut out, buf.len()) {
            Ok(n) => {
                buf[..n].copy_from_slice(&out[..n]);
                Ok(n)
            }
            Err(error) => {
                self.failure = Some(error);
                Err(io::Error::other("source failed"))
            }
        }
    }
}

fn convert<S>(
    source: &mut S,
    refill: fn(&mut S, &mut Vec<u8>, usize) -> Result<usize, Error>,
    encoded_column: Option<&str>,
) -> Result<String, Error> {
    let mut stream = Refilled { source, refill, failure: None };
    let mut reader = csv::ReaderBuilder::new().has_headers(false).from_reader(&mut stream);
    let mut headers: Vec<String> = Vec::new();
    let mut first = true;
    let mut encoded_index: Option<usize> = None;
    let mut hash_name = String::new();
    let mut converted: Vec<Value> = Vec::new();
    let mut records = reader.records();
    while let Some(row) = records.next() {
        let row: Vec<String> = match row {
            Ok(row) => row.iter().map(str::to_string).collect(),
            Err(error) => return Err(Error::Csv(error)),
        };
        if first {
            first = false;
            headers = row;
            let mut names: BTreeMap<String, bool> = BTreeMap::new();
            for (i, name) in headers.iter().enumerate() {
                if names.contains_key(name) {
                    return Err(Error::Invalid);
                }
                names.insert(name.clone(), true);
                if encoded_column == Some(name.as_str()) {
                    encoded_index = Some(i);
                }
            }
            if let Some(column) = encoded_column {
                if encoded_index.is_none() {
                    return Err(Error::Invalid);
                }
                hash_name = format!("{column}_sha256");
                if names.contains_key(&hash_name) {
                    return Err(Error::Invalid);
                }
            }
            continue;
        }
        if row.len() != headers.len() {
            return Err(Error::Invalid);
        }
        let mut object = Map::new();
        let mut digest = String::new();
        for j in 0..headers.len() {
            if encoded_index == Some(j) {
                let data = match STANDARD.decode(&row[j]) {
                    Ok(data) => data,
                    Err(error) => return Err(Error::Encoding(error, row[j].clone())),
                };
                let canonical = STANDARD.encode(&data);
                digest = Sha256::digest(&data).iter().map(|b| format!("{b:02x}")).collect();
                object.insert(headers[j].clone(), Value::String(canonical));
            } else {
                object.insert(headers[j].clone(), Value::String(row[j].clone()));
            }
        }
        if encoded_index.is_some() {
            object.insert(hash_name.clone(), Value::String(digest));
        }
        converted.push(Value::Object(object));
    }
    drop(records);
    drop(reader);
    if let Some(error) = stream.failure.take() {
        return Err(error);
    }
    let output = serde_json::to_string(&Value::Array(converted)).map_err(|_| Error::Invalid)?;
    Ok(format!("{output}\n"))
}

pub struct Flag {
    name: &'static str,
    short: Option<char>,
    value: bool,
    help: &'static str,
}

pub struct Parsed {
    flags: BTreeMap<String, Vec<String>>,
    positional: Vec<String>,
}

fn specs() -> Vec<Flag> {
    vec![
        Flag { name: "help", short: Some('h'), value: false, help: "show usage" },
        Flag {
            name: "base64-field",
            short: Some('b'),
            value: true,
            help: "decode/re-encode this field and add its _sha256 digest",
        },
    ]
}

fn parse(args: &[String], specs: &[Flag]) -> Result<Parsed, ParseError> {
    let mut parsed = Parsed { flags: BTreeMap::new(), positional: vec![] };
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        let found = if let Some(long) = arg.strip_prefix("--") {
            let (name, inline) = match long.split_once('=') {
                Some((n, v)) => (n, Some(v.to_string())),
                None => (long, None),
            };
            specs.iter().find(|f| f.name == name).map(|f| (f, inline))
        } else if arg.len() == 2 && arg.starts_with('-') && arg != "--" {
            let c = arg.chars().nth(1);
            specs.iter().find(|f| f.short == c).map(|f| (f, None))
        } else {
            parsed.positional.push(arg.clone());
            i += 1;
            continue;
        };
        let Some((flag, inline)) = found else {
            return Err(ParseError::UnknownFlag(i));
        };
        let value = match (flag.value, inline) {
            (true, Some(v)) => v,
            (true, None) => {
                i += 1;
                args.get(i).cloned().ok_or(ParseError::MissingValue(i - 1))?
            }
            (false, Some(_)) => return Err(ParseError::UnexpectedValue(i)),
            (false, None) => String::new(),
        };
        parsed.flags.entry(flag.name.to_string()).or_default().push(value);
        i += 1;
    }
    Ok(parsed)
}

fn usage(program: &str, specs: &[Flag]) -> String {
    let mut out = format!("usage: {program} [flags] [file]\n");
    for f in specs {
        let short = f.short.map(|c| format!("-{c}, ")).unwrap_or_default();
        out.push_str(&format!("  {short}--{}  {}\n", f.name, f.help));
    }
    out
}

fn emit(text: &str) -> Result<(), Error> {
    use std::io::Write;
    io::stdout().write_all(text.as_bytes()).map_err(Error::Output)
}

fn run(args: &[String]) -> Result<(), Error> {
    let parsed = parse(args, &specs()).map_err(Error::Flags)?;
    if parsed.flags.contains_key("help") {
        return emit(&usage("csvjson", &specs()));
    }
    if parsed.positional.len() > 1 {
        return Err(Error::Invalid);
    }
    let column: Option<String> = parsed.flags.get("base64-field").map(|v| v[v.len() - 1].clone());
    let column = column.as_deref();
    let output = if parsed.positional.is_empty() {
        let mut source = MemorySource {
            data: b"name,note\nAda,\"hello, sheep\"\nLin,\"a \"\"quote\"\"\"\n".to_vec(),
            offset: 0,
        };
        convert(&mut source, memory_refill, column)?
    } else if parsed.positional[0] == "-" {
        let mut source = InputSource { buffer: BufReader::with_capacity(4096, io::stdin()) };
        convert(&mut source, input_refill, column)?
    } else {
        let file = File::open(&parsed.positional[0]).map_err(Error::Open)?;
        let mut source = FileSource { buffer: BufReader::with_capacity(4096, file) };
        convert(&mut source, file_refill, column)?
    };
    emit(&output)
}

fn describe(error: &Error) -> String {
    match error {
        Error::Flags(ParseError::UnknownFlag(index)) => format!("unknown flag at argument {index}"),
        Error::Flags(ParseError::MissingValue(index)) => format!("missing flag value at argument {index}"),
        Error::Flags(ParseError::UnexpectedValue(index)) => {
            format!("unexpected flag value at argument {index}")
        }
        Error::Csv(error) => match error.kind() {
            csv::ErrorKind::UnequalLengths { pos, .. } => {
                format!("unequal CSV width at byte {}", pos.as_ref().map_or(0, |p| p.byte()))
            }
            csv::ErrorKind::Utf8 { .. } => "CSV field contains invalid UTF-8".into(),
            _ => "malformed CSV".into(),
        },
        Error::Encoding(_, value) => format!("invalid base64 field: {value}"),
        Error::Input(_) => "input read failed".into(),
        Error::Files(_) => "file read failed".into(),
        Error::Open(_) => "file could not be opened".into(),
        Error::Output(_) => "output failed".into(),
        Error::Invalid => "invalid CSV headers, row width, or arguments".into(),
    }
}

pub fn main(args: &[String]) -> Result<(), Error> {
    match run(args) {
        Ok(()) => Ok(()),
        Err(error) => {
            let mut entry = Map::new();
            entry.insert("level".into(), Value::String("error".into()));
            entry.insert("message".into(), Value::String("csvjson failed".into()));
            entry.insert("reason".into(), Value::String(describe(&error)));
            eprintln!("{}", Value::Object(entry));
            Err(Error::Invalid)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_quoted_csv_and_hashes_base64() {
        let mut source = MemorySource { data: b"id,body\n1,\"YWJj\"\n".to_vec(), offset: 0 };
        let output = convert(&mut source, memory_refill, Some("body")).ok().unwrap();
        assert_eq!(
            output,
            "[{\"body\":\"YWJj\",\"body_sha256\":\"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad\",\"id\":\"1\"}]\n"
        );
    }

    #[test]
    fn rejects_duplicate_headers_and_width_mismatches() {
        for csv in [&b"a,a\n1,2\n"[..], &b"a,b\n1\n"[..]] {
            let mut source = MemorySource { data: csv.to_vec(), offset: 0 };
            assert!(convert(&mut source, memory_refill, None).is_err());
        }
        let mut source = MemorySource { data: b"body,body_sha256\nYWJj,x\n".to_vec(), offset: 0 };
        assert!(convert(&mut source, memory_refill, Some("body")).is_err());
    }
}
