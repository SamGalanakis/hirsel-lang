//! Structured candidate client keeps stdin/stdout as JSON and never checks an
//! unedited disk tree when transport fails.
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
};
struct Process(std::process::Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn command(root: &std::path::Path) -> Command {
    let mut c = Command::new(std::path::absolute(env!("CARGO_BIN_EXE_hirsel")).unwrap());
    c.current_dir(root)
        .env("XDG_RUNTIME_DIR", root)
        .env("HIRSEL_DAEMON", "auto")
        .env("HIRSEL_RESULTS", "none@1");
    c
}
fn exchange(input: &mut impl Write, output: &mut impl BufRead, mut request: Value) -> Value {
    request["version"] = 1.into();
    writeln!(input, "{request}").unwrap();
    input.flush().unwrap();
    let mut line = String::new();
    assert!(
        output.read_line(&mut line).unwrap() > 0,
        "candidate client ended without a reply"
    );
    let reply: Value = serde_json::from_str(&line).unwrap();
    assert!(reply.get("error").is_none(), "{reply}");
    reply["ok"].clone()
}
#[test]
fn candidate_transport_receipts_duplicates_cancel_and_no_disk_mutation() {
    let root = std::env::temp_dir().join(format!(
        "hirsel-candidate-cli-{}-{}",
        std::process::id(),
        super::NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&root).unwrap();
    let original = "pub fn main()->u64{1}\n";
    std::fs::write(root.join("main.hir"), original).unwrap();
    std::fs::write(root.join("hirsel.json"),r#"{"schema":1,"package":{"name":"app","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
    let mut server = Process(
        command(&root)
            .args(["serve", "--workspace", ".", "--idle", "30"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut log = BufReader::new(server.0.stderr.take().unwrap());
    let mut ready = String::new();
    log.read_line(&mut ready).unwrap();
    assert!(ready.contains("serving"), "{ready}");
    let mut client = Process(
        command(&root)
            .args(["candidate", "--workspace", "."])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut input = client.0.stdin.take().unwrap();
    let mut output = BufReader::new(client.0.stdout.take().unwrap());
    let base = exchange(
        &mut input,
        &mut output,
        json!({"op":"capture","entry":"hirsel.json"}),
    );
    // Another connection cannot enter argv dispatch or see these handles.
    let mut contender = command(&root)
        .args(["candidate", "--workspace", "."])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    writeln!(
        contender.stdin.take().unwrap(),
        "{}",
        json!({"version":1,"op":"counters"})
    )
    .unwrap();
    let refused = contender.wait_with_output().unwrap();
    assert_eq!(refused.status.code(), Some(3));
    assert!(refused.stdout.is_empty());
    let source = &base["sources"][0];
    let file = source["file"].clone();
    let edit = json!({"file":file,"expected":source["expected"],"edits":[{"start":0,"end":source["bytes"],"replacement":"pub fn main()->u64{2}\n"}]});
    let mut leases = vec![];
    for id in 1..=8 {
        leases.push(
            exchange(
                &mut input,
                &mut output,
                json!({"op":"submit","id":id,"base":base["base"],"edits":[edit]}),
            )["lease"]
                .clone(),
        );
    }
    let mut digest = None;
    for lease in &leases {
        loop {
            let receipt = exchange(
                &mut input,
                &mut output,
                json!({"op":"result","lease":lease}),
            );
            if receipt["pending"].as_bool() == Some(true) {
                std::thread::yield_now();
                continue;
            }
            assert_eq!(receipt["completion"], "Complete");
            assert_eq!(receipt["verdict"], "Pass");
            if let Some(expected) = &digest {
                assert_eq!(expected, &receipt["checked_products"]);
            } else {
                digest = Some(receipt["checked_products"].clone());
            }
            break;
        }
    }
    let counts = exchange(&mut input, &mut output, json!({"op":"counters"}));
    assert_eq!(counts["frozen_loads"], 1);
    assert_eq!(counts["completed_results"], 1);
    assert_eq!(counts["forks"], 0);
    let promotion = exchange(
        &mut input,
        &mut output,
        json!({"op":"promote","id":1,"base":base["base"],"lease":leases[0]}),
    );
    assert_eq!(promotion["promotion"], "Promoted");
    assert_eq!(
        std::fs::read_to_string(root.join("main.hir")).unwrap(),
        original
    );
    for lease in leases {
        exchange(
            &mut input,
            &mut output,
            json!({"op":"release","lease":lease}),
        );
    }
    drop(input);
    assert!(client.0.wait().unwrap().success());
    assert!(server.0.wait().unwrap().success());
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn candidate_v2_transport_atomic_create_manifest_and_connection_latch() {
    let root = std::env::temp_dir().join(format!(
        "hirsel-candidate-v2-cli-{}-{}",
        std::process::id(),
        super::NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(root.join("mods")).unwrap();
    let original = "pub fn main()->u64{1}\n";
    std::fs::write(root.join("main.hir"), original).unwrap();
    std::fs::write(root.join("hirsel.json"),r#"{"schema":1,"package":{"name":"app","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
    let mut server = Process(
        command(&root)
            .args(["serve", "--workspace", ".", "--idle", "1"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut ready = String::new();
    BufReader::new(server.0.stderr.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    assert!(ready.contains("serving"), "{ready}");
    let mut client = Process(
        command(&root)
            .args(["candidate", "--workspace", "."])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut input = client.0.stdin.take().unwrap();
    let mut output = BufReader::new(client.0.stdout.take().unwrap());
    writeln!(input, "{{\"version\":2,\"op\":\"head\",\"op\":\"head\"}}").unwrap();
    input.flush().unwrap();
    let mut invalid = String::new();
    output.read_line(&mut invalid).unwrap();
    let invalid: Value = serde_json::from_str(&invalid).unwrap();
    assert_eq!(invalid["version"], 2);
    assert!(
        invalid["error"]
            .as_str()
            .unwrap()
            .starts_with("InvalidRequest")
    );

    let mut exchange = |request: Value| -> Value {
        writeln!(input, "{request}").unwrap();
        input.flush().unwrap();
        let mut line = String::new();
        assert!(output.read_line(&mut line).unwrap() > 0);
        serde_json::from_str(&line).unwrap()
    };
    let base = exchange(
        json!({"version":2,"op":"capture","entry":"hirsel.json","roots":[{"id":"app","path":"","editable":true}]}),
    );
    assert!(base.get("ok").is_some(), "{base}");
    let base = base["ok"].clone();
    assert_eq!(
        exchange(json!({"version":1,"op":"head"}))["error"],
        "UnsupportedVersion"
    );
    let facts = exchange(
        json!({"version":2,"op":"inspect","base":base["base"],"paths":[{"root":"app","path":"mods/new.hir"},{"root":"app","path":"main.hir"},{"root":"app","path":"hirsel.json"}]}),
    );
    let facts = facts["ok"]["nodes"].as_array().unwrap();
    let manifest = json!({"schema":1,"package":{"name":"app","version":"1"},"sources":[{"path":"main.hir","module":[]},{"path":"mods/new.hir","module":["new"]}],"dependencies":[],"entry":{"module":[],"name":"main"}});
    let request = json!({"version":2,"op":"submit","base":base["base"],"token":base["token"],"id":1,"edits":[
        {"kind":"create_source","path":{"root":"app","path":"mods/new.hir"},"expected":facts[0]["expected"],"text":"pub fn value()->u64{2}\n"},
        {"kind":"replace_source","path":{"root":"app","path":"main.hir"},"expected":facts[1]["expected"],"text":"pub fn main()->u64{new.value()}\n"},
        {"kind":"manifest","action":"replace","path":{"root":"app","path":"hirsel.json"},"expected":facts[2]["expected"],"text":manifest.to_string()},
    ]});
    let submitted = exchange(request);
    let lease = submitted["ok"]["lease"].as_u64().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let receipt = loop {
        let r = exchange(json!({"version":2,"op":"result","lease":lease}));
        if r["ok"]["pending"] != true {
            break r;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    };
    assert_eq!(receipt["ok"]["verdict"], "Pass", "{receipt}");
    assert_eq!(receipt["ok"]["scope_complete"], true);
    assert_eq!(
        exchange(json!({"version":2,"op":"promote","id":1,"lease":lease,"base":base["base"]}))["ok"]
            ["promotion"],
        "Promoted"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("main.hir")).unwrap(),
        original
    );
    assert!(!root.join("mods/new.hir").exists());
    drop(exchange);
    drop(input);
    assert!(client.0.wait().unwrap().success());
    assert!(server.0.wait().unwrap().success());
    std::fs::remove_dir_all(root).unwrap();
}
