use super::*;
use std::time::{Duration, SystemTime};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "hirsel-program-cache-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn age(path: &Path, seconds: u64) {
    let file = std::fs::File::options().append(true).open(path).unwrap();
    file.set_modified(SystemTime::now() - Duration::from_secs(seconds))
        .unwrap();
}

#[test]
fn an_entry_reads_back_and_a_missing_one_is_a_miss() {
    let dir = scratch("roundtrip");
    let cache = ProgramCache::at(dir.clone());
    assert_eq!(cache.read("f-a.json"), None);
    cache.put("f-a.json", b"facts");
    assert_eq!(cache.read("f-a.json").as_deref(), Some(&b"facts"[..]));
    // Writers leave no temporary file behind.
    let names: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names, ["f-a.json"]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_hit_marks_its_entry_recently_used() {
    let dir = scratch("touch");
    let cache = ProgramCache::at(dir.clone());
    cache.put("i-a.so", b"image");
    age(&dir.join("i-a.so"), 3600);
    let before = std::fs::metadata(dir.join("i-a.so"))
        .unwrap()
        .modified()
        .unwrap();
    cache.get("i-a.so").unwrap();
    let after = std::fs::metadata(dir.join("i-a.so"))
        .unwrap()
        .modified()
        .unwrap();
    assert!(after > before);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn reclaiming_removes_the_least_recently_used_entries_past_the_cap() {
    let dir = scratch("reclaim");
    let cache = ProgramCache::at(dir.clone());
    for (name, seconds) in [("i-old.so", 3000), ("i-mid.so", 2000), ("i-new.so", 1000)] {
        cache.put(name, &[0; 100]);
        age(&dir.join(name), seconds);
    }
    cache.reclaim(250);
    assert!(!dir.join("i-old.so").exists());
    assert!(dir.join("i-mid.so").exists() && dir.join("i-new.so").exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn reclaiming_keeps_entries_in_use() {
    let dir = scratch("held");
    let cache = ProgramCache::at(dir.clone());
    cache.put("i-held.so", &[0; 100]);
    age(&dir.join("i-held.so"), 3000);
    cache.put("i-fresh.so", &[0; 100]);
    // A run's shared hold, as `runner::hold` takes it.
    let hold = std::fs::File::open(dir.join("i-held.so")).unwrap();
    hold.lock_shared().unwrap();
    cache.reclaim(0);
    assert!(dir.join("i-held.so").exists(), "held");
    assert!(
        dir.join("i-fresh.so").exists(),
        "used within the grace period"
    );
    drop(hold);
    cache.reclaim(0);
    assert!(!dir.join("i-held.so").exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn reclaiming_removes_abandoned_temporary_files() {
    let dir = scratch("abandoned");
    let cache = ProgramCache::at(dir.clone());
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(".i-a.so.1.0.tmp"), b"partial").unwrap();
    std::fs::write(dir.join(".i-b.so.2.0.tmp"), b"writing").unwrap();
    age(&dir.join(".i-a.so.1.0.tmp"), 7200);
    cache.reclaim(u64::MAX);
    assert!(!dir.join(".i-a.so.1.0.tmp").exists());
    assert!(dir.join(".i-b.so.2.0.tmp").exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn launch_facts_read_back_as_written() {
    use hirsel_run_config::Root;
    use hirsel_run_schema::{CodeReach, LaunchKind, ProgramRevision};
    for (kind, roots, code) in [
        (
            LaunchKind::Main,
            vec![Root::Io, Root::Args, Root::Env],
            CodeReach::Safe,
        ),
        (
            LaunchKind::Test,
            vec![Root::Test, Root::Unsafe],
            CodeReach::Escapes(vec!["app@1/main".into(), "app@1/ffi".into()]),
        ),
    ] {
        let facts = LaunchFacts {
            revision: ProgramRevision::recorded("app@1/main", [7; 32]),
            kind,
            roots,
            code,
        };
        assert_eq!(LaunchFacts::decode(&facts.encode()), Some(facts));
    }
    assert_eq!(LaunchFacts::decode(b"{\"schema\":2}"), None);
    assert_eq!(LaunchFacts::decode(b"not json"), None);
}

#[test]
fn keys_name_their_domain_and_parts() {
    let a = key("hirsel.test.v1", [b"a".as_slice(), b"b"]);
    assert_eq!(a.len(), 64);
    assert_ne!(a, key("hirsel.test.v1", [b"ab".as_slice()]));
    assert_ne!(a, key("hirsel.test.v2", [b"a".as_slice(), b"b"]));
}
