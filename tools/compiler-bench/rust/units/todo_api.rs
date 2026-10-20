// A JSON todo API. Each connection is served by its own thread; the todo list lives in a
// Mutex, and one writer thread saves every change atomically to data/todos.json.
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Todo {
    id: u64,
    title: String,
    done: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Store {
    next_id: u64,
    version: u64,
    todos: Vec<Todo>,
}

// A snapshot of the store after a change, and where to report whether it was saved.
pub struct SaveJob {
    version: u64,
    text: String,
    reply: Sender<bool>,
}

// What a request does to the store, decided before taking the lock.
pub enum Action {
    Index,
    Create(String),
    Show(u64),
    Update(u64, bool),
    Remove(u64),
}

pub struct Reply {
    status: u16,
    body: String,
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

const STATE: &str = "todos.json";

// ---- JSON ----
fn text_value(s: &str) -> Value {
    Value::String(s.to_string())
}

// A todo's JSON object and its decoding are derived from Todo's shape.
fn todo_value(t: &Todo) -> Value {
    serde_json::to_value(t).unwrap_or(Value::Null)
}

fn todo_from(value: &Value) -> Result<Todo, String> {
    Todo::deserialize(value).map_err(|e| e.to_string())
}

fn todo_json(t: &Todo) -> String {
    serde_json::to_string(&todo_value(t)).unwrap_or_else(|_| "{}".to_string())
}

fn list_value(todos: &[Todo]) -> Value {
    let mut items = Vec::new();
    for t in todos {
        items.push(todo_value(t));
    }
    Value::Array(items)
}

fn store_json(st: &Store) -> String {
    let mut fields = Map::new();
    fields.insert("next_id".into(), Value::String(st.next_id.to_string()));
    fields.insert("todos".into(), list_value(&st.todos));
    serde_json::to_string(&Value::Object(fields)).unwrap_or_else(|_| "{}".to_string())
}

fn error_json(message: &str) -> String {
    let mut fields = Map::new();
    fields.insert("error".into(), text_value(message));
    serde_json::to_string(&Value::Object(fields)).unwrap_or_else(|_| "{}".to_string())
}

fn text_field(fields: &Map<String, Value>, key: &str) -> Option<String> {
    match fields.get(key) {
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    }
}

fn bool_field(fields: &Map<String, Value>, key: &str) -> Option<bool> {
    match fields.get(key) {
        Some(Value::Bool(b)) => Some(*b),
        _ => None,
    }
}

fn u64_field(fields: &Map<String, Value>, key: &str) -> Option<u64> {
    match fields.get(key) {
        Some(Value::String(n)) => n.parse().ok(),
        Some(Value::Number(n)) => n.as_u64(),
        _ => None,
    }
}

fn parse_object(body: &[u8]) -> Option<Map<String, Value>> {
    let text = std::str::from_utf8(body).ok()?;
    match serde_json::from_str(text) {
        Ok(Value::Object(fields)) => Some(fields),
        _ => None,
    }
}

fn parse_store(text: &str) -> Result<Store, ServerError> {
    let Ok(Value::Object(fields)) = serde_json::from_str::<Value>(text) else {
        return Err(ServerError::Corrupt("state is not a JSON object".into()));
    };
    let Some(next_id) = u64_field(&fields, "next_id") else {
        return Err(ServerError::Corrupt("next_id".into()));
    };
    let Some(Value::Array(items)) = fields.get("todos") else {
        return Err(ServerError::Corrupt("todos".into()));
    };
    let mut todos = Vec::new();
    for item in items {
        match todo_from(item) {
            Ok(decoded) => todos.push(decoded),
            Err(message) => return Err(ServerError::Corrupt(message)),
        }
    }
    Ok(Store { next_id, version: 0, todos })
}

// ---- persistence ----
fn load(dir: &Path) -> Result<Store, ServerError> {
    match fs::read(dir.join(STATE)) {
        Ok(data) => {
            let Ok(text) = String::from_utf8(data) else {
                return Err(ServerError::Corrupt("state is not UTF-8".into()));
            };
            parse_store(&text)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            Ok(Store { next_id: 1, version: 0, todos: vec![] })
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

// Saves arrive in lock order up to channel interleaving; a later version already
// written makes an earlier one redundant, so the file only moves forward.
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
fn not_found() -> Reply {
    Reply { status: 404, body: error_json("not found"), allow: String::new(), save: None }
}

fn bad_request(message: &str) -> Reply {
    Reply { status: 400, body: error_json(message), allow: String::new(), save: None }
}

fn method_not_allowed(allow: &str) -> Reply {
    Reply {
        status: 405,
        body: error_json("method not allowed"),
        allow: allow.to_string(),
        save: None,
    }
}

fn unsupported_media() -> Reply {
    Reply {
        status: 415,
        body: error_json("Content-Type must be application/json"),
        allow: String::new(),
        save: None,
    }
}

fn find(todos: &[Todo], id: u64) -> Option<usize> {
    for (i, t) in todos.iter().enumerate() {
        if t.id == id {
            return Some(i);
        }
    }
    None
}

// The caller has already bumped the version; the snapshot goes to the writer thread.
fn snapshot_reply(st: &Store, status: u16, body: String) -> Reply {
    Reply { status, body, allow: String::new(), save: Some((st.version, store_json(st))) }
}

fn apply(st: &mut Store, action: Action) -> Reply {
    match action {
        Action::Index => Reply {
            status: 200,
            body: serde_json::to_string(&list_value(&st.todos)).unwrap_or_else(|_| "[]".into()),
            allow: String::new(),
            save: None,
        },
        Action::Create(title) => {
            let item = Todo { id: st.next_id, title, done: false };
            st.next_id += 1;
            let body = todo_json(&item);
            st.todos.push(item);
            st.version += 1;
            snapshot_reply(st, 201, body)
        }
        Action::Show(id) => match find(&st.todos, id) {
            Some(i) => Reply {
                status: 200,
                body: todo_json(&st.todos[i]),
                allow: String::new(),
                save: None,
            },
            None => not_found(),
        },
        Action::Update(id, done) => match find(&st.todos, id) {
            Some(i) => {
                st.todos[i].done = done;
                let body = todo_json(&st.todos[i]);
                st.version += 1;
                snapshot_reply(st, 200, body)
            }
            None => not_found(),
        },
        Action::Remove(id) => match find(&st.todos, id) {
            Some(i) => {
                st.todos.remove(i);
                st.version += 1;
                snapshot_reply(st, 204, String::new())
            }
            None => not_found(),
        },
    }
}

// ---- HTTP/1.1 requests and responses ----
pub struct Request {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
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
    let mut headers = Vec::new();
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
        headers.push((name.trim().to_string(), value.trim().to_string()));
    }
    let length = match header(&headers, "content-length") {
        Some(n) => n.parse::<usize>().map_err(|_| invalid("content-length"))?,
        None => 0,
    };
    if length > max_body {
        return Err(invalid("body too large"));
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    Ok(Request { method, target, headers, body })
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        415 => "Unsupported Media Type",
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

// `application/json`, in any case and with or without parameters such as a charset.
fn json_content(request: &Request) -> bool {
    let value = match header(&request.headers, "content-type") {
        Some(v) => v.to_ascii_lowercase(),
        None => return false,
    };
    let media = match value.split_once(';') {
        Some((media, _)) => media,
        None => &value,
    };
    media.trim() == "application/json"
}

// Decide the action from method, path and body; an error is the reply to send.
fn route(request: &Request) -> Result<Action, Reply> {
    let target = match request.target.split_once('?') {
        Some((target_path, _)) => target_path,
        None => &request.target,
    };
    let parts: Vec<&str> = target.split('/').collect();
    if parts.len() < 2 || !parts[0].is_empty() || parts[1] != "todos" || parts.len() > 3 {
        return Err(not_found());
    }
    if parts.len() == 2 {
        if request.method == "GET" {
            return Ok(Action::Index);
        }
        if request.method != "POST" {
            return Err(method_not_allowed("GET, POST"));
        }
        if !json_content(request) {
            return Err(unsupported_media());
        }
        let Some(fields) = parse_object(&request.body) else {
            return Err(bad_request("body must be a JSON object"));
        };
        return match text_field(&fields, "title") {
            Some(title) if title.is_empty() => Err(bad_request("title must not be empty")),
            Some(title) => Ok(Action::Create(title)),
            None => Err(bad_request("title must be a string")),
        };
    }
    let id = match parts[2].parse::<u64>() {
        Ok(n) => n,
        Err(_) => return Err(not_found()),
    };
    if request.method == "GET" {
        return Ok(Action::Show(id));
    }
    if request.method == "DELETE" {
        return Ok(Action::Remove(id));
    }
    if request.method != "PATCH" {
        return Err(method_not_allowed("GET, PATCH, DELETE"));
    }
    if !json_content(request) {
        return Err(unsupported_media());
    }
    let Some(fields) = parse_object(&request.body) else {
        return Err(bad_request("body must be a JSON object"));
    };
    match bool_field(&fields, "done") {
        Some(done) => Ok(Action::Update(id, done)),
        None => Err(bad_request("done must be a boolean")),
    }
}

// ---- connections ----
fn send_reply(connection: &TcpStream, reply: &Reply) -> io::Result<()> {
    let mut headers = Vec::new();
    if !reply.body.is_empty() {
        headers.push(("Content-Type", "application/json"));
    }
    if !reply.allow.is_empty() {
        headers.push(("Allow", reply.allow.as_str()));
    }
    respond(connection, reply.status, &headers, reply.body.as_bytes())
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
    let saved = match saves.send(SaveJob { version, text: snapshot, reply: done_tx }) {
        Ok(()) => done_rx.recv().unwrap_or(false),
        Err(_) => false,
    };
    if saved {
        reply
    } else {
        Reply { status: 500, body: error_json("could not save"), allow: String::new(), save: None }
    }
}

fn serve(state: &Mutex<Store>, saves: Sender<SaveJob>, stream: TcpStream) {
    let reply = match receive_request(&stream, 16384, 65536) {
        Ok(request) => handle(state, &saves, &request),
        Err(_) => bad_request("malformed HTTP request"),
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
    let state = Mutex::new(store);
    let (saves, jobs) = mpsc::channel();
    let listener = TcpListener::bind(("127.0.0.1", port)).map_err(ServerError::Network)?;
    let bound = listener.local_addr().map_err(ServerError::Network)?;
    println!("listening on 127.0.0.1:{}", bound.port());
    thread::scope(|s| {
        s.spawn(|| write_loop(dir.clone(), jobs));
        loop {
            let (stream, _) = listener.accept().map_err(ServerError::Network)?;
            let jobs_tx = saves.clone();
            let state = &state;
            s.spawn(move || serve(state, jobs_tx, stream));
        }
    })
}
