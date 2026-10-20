use hirsel_run_config::*;
use hirsel_run_schema::{LaunchKind, RunRefusal};

fn declaration() -> serde_json::Value {
    serde_json::json!({"object_stores":{"data":{"endpoint":"https://storage.example:9443", "bucket":"hirsel-data", "region":"auto", "prefix":"service/db", "cache_bytes":4096, "credentials":{"mount":"secrets","path":"data.json"}}}, "io":{"dirs":[{"path":"secrets","host":"/srv/credentials","access":"read"}]}})
}
fn bind(value: serde_json::Value) -> Result<ValidatedRun, RunRefusal> {
    RunConfig::bind_grants(
        &serde_json::to_vec(&value).unwrap(),
        [0; 32],
        "app@1/main".into(),
        LaunchKind::Main,
    )
}
#[test]
fn object_stores_project_consistent_grants_and_round_trip() {
    let run = bind(declaration()).unwrap();
    let io = run.record().io.as_ref().unwrap();
    let rule = &io.net.as_ref().unwrap().rules[0];
    assert_eq!(
        (
            rule.address.as_str(),
            rule.service.as_str(),
            rule.operation.as_str()
        ),
        ("storage.example", "9443", "connect_tls")
    );
    let env = run.record().env.as_ref().unwrap();
    assert_eq!(
        env["HIRSEL_S3_DATA_CREDENTIALS"].as_deref(),
        Some("data.json")
    );
    assert_eq!(
        env["HIRSEL_S3_DATA_ENDPOINT"].as_deref(),
        Some("https://storage.example:9443")
    );
    let again = RunConfig::validate(run.canonical()).unwrap();
    assert_eq!(again.canonical(), run.canonical());
    assert!(
        !String::from_utf8(run.canonical().to_vec())
            .unwrap()
            .contains("secret_key")
    );
}
#[test]
fn invalid_endpoints_credentials_and_projection_conflicts_refuse() {
    for endpoint in [
        "file:///etc/passwd",
        "https://user:secret@host",
        "https://storage.example/path",
        "http://127.1",
        "http://host:01",
        "https://host/?secret=x",
        "https://[storage.example]",
        "http://::1",
        "http://host:0",
        "https://host/#fragment",
    ] {
        let mut value = declaration();
        value["object_stores"]["data"]["endpoint"] = endpoint.into();
        assert!(bind(value).is_err(), "{endpoint}");
    }
    for path in ["../secret.json", "/secret.json", "a/../secret.json", ""] {
        let mut value = declaration();
        value["object_stores"]["data"]["credentials"]["path"] = path.into();
        assert!(bind(value).is_err(), "{path}");
    }
    let mut value = declaration();
    value["env"] = serde_json::json!({"HIRSEL_S3_DATA_BUCKET":"another"});
    assert!(bind(value).is_err());
    let mut value = declaration();
    value["object_stores"]["data"]["credentials"]["mount"] = "missing".into();
    assert!(bind(value).is_err());
}

#[test]
fn multiple_stores_have_separate_projections_and_exact_endpoint_authority() {
    let mut value = declaration();
    value["object_stores"]["other"] = value["object_stores"]["data"].clone();
    value["object_stores"]["other"]["bucket"] = "other-bucket".into();
    let run = bind(value).unwrap();
    let env = run.record().env.as_ref().unwrap();
    assert_eq!(
        env["HIRSEL_S3_OTHER_BUCKET"].as_deref(),
        Some("other-bucket")
    );
    assert_eq!(env["HIRSEL_S3_DATA_BUCKET"].as_deref(), Some("hirsel-data"));
    assert_eq!(run.net_grant().unwrap().rules.len(), 1);
    assert_eq!(
        run.record().io.as_ref().unwrap().wall_clock.as_deref(),
        Some("clock.wall.unix_ns@1")
    );
}

#[test]
fn duplicate_store_names_and_inline_secrets_are_rejected() {
    let entry = declaration()["object_stores"]["data"].to_string();
    let duplicates = format!("{{\"object_stores\":{{\"data\":{entry},\"data\":{entry}}}}}");
    assert!(
        RunConfig::bind_grants(
            duplicates.as_bytes(),
            [0; 32],
            "app@1/main".into(),
            LaunchKind::Main
        )
        .is_err()
    );
    let mut value = declaration();
    value["object_stores"]["data"]["secret_key"] = "inline".into();
    assert!(bind(value).is_err());
}
