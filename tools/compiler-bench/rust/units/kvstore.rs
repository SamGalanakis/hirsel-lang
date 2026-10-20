// A persistent key-value server: `GET`, `PUT` and `DELETE /kv/<key>` and `GET /kv`.
// Each connection is served by its own thread; the entries live in a Mutex, and one
// writer thread saves every change atomically to data/kv.json before the reply.
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

pub struct Store {
    version: u64,
    entries: BTreeMap<String, String>,
}

pub struct SaveJob {
    version: u64,
    text: String,
    reply: Sender<bool>,
}

pub enum Action {
    List,
    Read(String),
    Write(String, String),
    Delete(String),
}

pub struct Reply {
    status: u16,
    body: String,
    json: bool,
    allow: String,
    save: Option<(u64, String)>,
}

#[derive(Debug)]
pub enum ServerError {
    Network(io::Error),
    Files(io::Error),
    Corrupt(String),
    Usage(String),
}

const STATE: &str = "kv.json";

const MAX_VALUE: usize = 65536;

// ---- JSON ----
fn entries_json(entries: &BTreeMap<String, String>) -> String {
    let mut fields = Map::new();
    for (key, value) in entries {
        fields.insert(key.clone(), Value::String(value.clone()));
    }
    serde_json::to_string(&Value::Object(fields)).unwrap_or_else(|_| "{}".to_string())
}

fn parse_entries(text: &str) -> Result<BTreeMap<String, String>, ServerError> {
    let fields = match serde_json::from_str(text) {
        Ok(Value::Object(fields)) => fields,
        _ => return Err(ServerError::Corrupt("state is not a JSON object".into())),
    };
    let mut entries = BTreeMap::new();
    for (key, value) in fields {
        match value {
            Value::String(s) => {
                entries.insert(key, s);
            }
            _ => return Err(ServerError::Corrupt(format!("value of {key}"))),
        }
    }
    Ok(entries)
}

// ---- persistence ----
fn load(dir: &Path) -> Result<Store, ServerError> {
    match fs::read(dir.join(STATE)) {
        Ok(data) => {
            let text = match String::from_utf8(data) {
                Ok(s) => s,
                Err(_) => return Err(ServerError::Corrupt("state is not UTF-8".into())),
            };
            Ok(Store { version: 0, entries: parse_entries(&text)? })
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            Ok(Store { version: 0, entries: BTreeMap::new() })
        }
        Err(e) => Err(ServerError::Files(e)),
    }
}

fn save(dir: &Path, text: &str) -> io::Result<()> {
    let temporary = dir.join(format!("{STATE}.tmp"));
    let mut f = fs::File::create(&temporary)?;
    f.write_all(text.as_bytes())?;
    f.sync_all()?;
    fs::rename(temporary, dir.join(STATE))
}

// Saves arrive in lock order up to channel interleaving; a version already
// superseded on disk is acknowledged without writing, so the file only moves forward.
fn write_loop(dir: PathBuf, receiver: Receiver<SaveJob>) {
    let mut written: u64 = 0;
    while let Ok(SaveJob { version, text, reply }) = receiver.recv() {
        let mut ok = true;
        if version > written {
            ok = save(&dir, &text).is_ok();
            if ok {
                written = version;
            }
        }
        let _ = reply.send(ok);
    }
}

// ---- routing ----
fn plain(status: u16, body: &str) -> Reply {
    Reply { status, body: body.to_string(), json: false, allow: String::new(), save: None }
}

// The caller has already bumped the version; the snapshot goes to the writer thread.
fn changed(st: &Store, status: u16) -> Reply {
    Reply {
        status,
        body: String::new(),
        json: false,
        allow: String::new(),
        save: Some((st.version, entries_json(&st.entries))),
    }
}

fn apply(st: &mut Store, action: Action) -> Reply {
    match action {
        Action::List => Reply {
            status: 200,
            body: entries_json(&st.entries),
            json: true,
            allow: String::new(),
            save: None,
        },
        Action::Read(key) => match st.entries.get(&key) {
            Some(value) => plain(200, value),
            None => plain(404, "not found\n"),
        },
        Action::Write(key, value) => {
            let status = if st.entries.contains_key(&key) { 200 } else { 201 };
            st.entries.insert(key, value);
            st.version += 1;
            changed(st, status)
        }
        Action::Delete(key) => match st.entries.remove(&key) {
            Some(_) => {
                st.version += 1;
                changed(st, 204)
            }
            None => plain(404, "not found\n"),
        },
    }
}

fn method_not_allowed(allow: &str) -> Reply {
    Reply {
        status: 405,
        body: "method not allowed\n".into(),
        json: false,
        allow: allow.to_string(),
        save: None,
    }
}

fn percent_decode(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

// Decide the action from method, path and body; an error is the reply to send.
fn route(request: &Request) -> Result<Action, Reply> {
    let target = match request.target.split_once('?') {
        Some((target_path, _)) => target_path,
        None => &request.target,
    };
    let parts: Vec<&str> = target.split('/').collect();
    if parts.len() < 2 || !parts[0].is_empty() || parts[1] != "kv" || parts.len() > 3 {
        return Err(plain(404, "not found\n"));
    }
    if parts.len() == 2 {
        if request.method != "GET" {
            return Err(method_not_allowed("GET"));
        }
        return Ok(Action::List);
    }
    let key = match String::from_utf8(percent_decode(parts[2])) {
        Ok(k) => k,
        Err(_) => return Err(plain(400, "key must be UTF-8\n")),
    };
    if key.is_empty() {
        return Err(plain(400, "empty key\n"));
    }
    if request.method == "GET" {
        return Ok(Action::Read(key));
    }
    if request.method == "DELETE" {
        return Ok(Action::Delete(key));
    }
    if request.method != "PUT" {
        return Err(method_not_allowed("GET, PUT, DELETE"));
    }
    match String::from_utf8(request.body.clone()) {
        Ok(value) => Ok(Action::Write(key, value)),
        Err(_) => Err(plain(400, "value must be UTF-8\n")),
    }
}

// ---- HTTP/1.1 requests and responses ----
pub struct Request {
    method: String,
    target: String,
    body: Vec<u8>,
}

fn receive_request(stream: &TcpStream, max_head: usize, max_body: usize) -> io::Result<Request> {
    let invalid = |m: &str| io::Error::new(io::ErrorKind::InvalidData, m.to_string());
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut head = line.len();
    let mut parts = line.split_whitespace();
    let (Some(method), Some(target), Some(_version)) = (parts.next(), parts.next(), parts.next())
    else {
        return Err(invalid("request line"));
    };
    let (method, target) = (method.to_string(), target.to_string());
    let mut length = 0;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line)?;
        head += line.len();
        if head > max_head {
            return Err(invalid("head too large"));
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(invalid("header"));
        };
        if name.trim().eq_ignore_ascii_case("content-length") {
            length = value.trim().parse().map_err(|_| invalid("content-length"))?;
        }
    }
    if length > max_body {
        return Err(invalid("body too large"));
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    Ok(Request { method, target, body })
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Internal Server Error",
    }
}

fn respond(mut stream: &TcpStream, status: u16, headers: &[(&str, &str)], body: &[u8]) -> io::Result<()> {
    let mut out = format!("HTTP/1.1 {status} {}\r\nConnection: close\r\n", reason(status));
    for (name, value) in headers {
        out.push_str(&format!("{name}: {value}\r\n"));
    }
    out.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
    stream.write_all(out.as_bytes())?;
    stream.write_all(body)
}

// ---- connections ----
fn send_reply(connection: &TcpStream, reply: &Reply) -> io::Result<()> {
    let mut headers = Vec::new();
    if !reply.body.is_empty() {
        let media = if reply.json { "application/json" } else { "text/plain; charset=utf-8" };
        headers.push(("Content-Type", media));
    }
    if !reply.allow.is_empty() {
        headers.push(("Allow", reply.allow.as_str()));
    }
    respond(connection, reply.status, &headers, reply.body.as_bytes())
}

fn queue(saves: &Sender<SaveJob>, version: u64, text: String, reply: Sender<bool>) -> bool {
    saves.send(SaveJob { version, text, reply }).is_ok()
}

fn handle(state: &Mutex<Store>, saves: &Sender<SaveJob>, request: &Request) -> Reply {
    let action = match route(request) {
        Ok(a) => a,
        Err(reply) => return reply,
    };
    let mut reply = {
        let mut store = state.lock().unwrap_or_else(|e| e.into_inner());
        apply(&mut store, action)
    };
    let Some((version, snapshot)) = reply.save.take() else {
        return reply;
    };
    let (done_tx, done_rx) = mpsc::channel();
    let saved = if queue(saves, version, snapshot, done_tx) {
        done_rx.recv().unwrap_or(false)
    } else {
        false
    };
    if saved { reply } else { plain(500, "could not save\n") }
}

fn serve(state: &Mutex<Store>, saves: Sender<SaveJob>, stream: TcpStream) {
    let reply = match receive_request(&stream, 16384, MAX_VALUE) {
        Ok(request) => handle(state, &saves, &request),
        Err(_) => plain(400, "malformed HTTP request\n"),
    };
    let _ = send_reply(&stream, &reply);
}

fn port_arg(args: &[String]) -> Result<u16, ServerError> {
    let Some(text) = args.first() else {
        return Ok(8080);
    };
    text.parse()
        .map_err(|_| ServerError::Usage("port must be a number from 0 to 65535".into()))
}

// Arguments: [port], default 8080; port 0 picks a free port. Prints the bound address.
pub fn main(args: &[String]) -> Result<(), ServerError> {
    let port = port_arg(args)?;
    let dir = PathBuf::from("data");
    let store = load(&dir)?;
    let listener = TcpListener::bind(("127.0.0.1", port)).map_err(ServerError::Network)?;
    let bound = listener.local_addr().map_err(ServerError::Network)?;
    println!("listening on 127.0.0.1:{}", bound.port());
    let state = Mutex::new(store);
    let (saves, jobs) = mpsc::channel();
    thread::scope(|s| {
        s.spawn(|| write_loop(dir.clone(), jobs));
        let state = &state;
        loop {
            let (stream, _) = listener.accept().map_err(ServerError::Network)?;
            let jobs_tx = saves.clone();
            s.spawn(move || serve(state, jobs_tx, stream));
        }
    })
}
