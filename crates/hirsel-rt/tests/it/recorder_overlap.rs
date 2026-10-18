//! Host admission controls: every overlap refusal precedes creation, truncation
//! and entry; the same grants with isolated recorder storage enter successfully.
use hirsel_rt::host_paths::{PinnedDirectory, RecorderDestination, RecorderDirectory};
use hirsel_rt::io_real::RealIo;
use hirsel_run_config::{HostAuthority, RevocationEvidence, RunConfig, RunRefusal};
use std::fs::{self, File};
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

struct Scratch(PathBuf);
impl Scratch {
    fn new(name: &str) -> Self {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("recorder-{name}-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        // Host roots in a run configuration are canonical absolute paths.
        let root = fs::canonicalize(root).unwrap();
        for name in ["work", "work/child", "tapes", "other", "other/child"] {
            fs::create_dir(root.join(name)).unwrap();
        }
        Self(root)
    }
    fn path(&self, path: &str) -> PathBuf {
        self.0.join(path)
    }
    fn directory(&self, path: &str) -> PinnedDirectory {
        PinnedDirectory::open(&self.path(path)).unwrap()
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn refused(path: &Path, grants: &[(&str, &PinnedDirectory)], mount: &str) {
    assert_eq!(
        RecorderDestination::pin(path)
            .unwrap()
            .open(grants)
            .unwrap_err(),
        RunRefusal::RecorderOverlap {
            mount: mount.into()
        }
    );
}
fn accepted(path: &Path, grants: &[(&str, &PinnedDirectory)]) {
    let mut file = RecorderDestination::pin(path)
        .unwrap()
        .open(grants)
        .unwrap();
    use std::io::Write;
    file.write_all(b"accepted").unwrap();
    assert_eq!(fs::read(path).unwrap(), b"accepted");
}

#[test]
fn inside_grant_and_grant_inside_recorder_directory_are_refused_with_isolated_twins() {
    let scratch = Scratch::new("containment");
    let work = scratch.directory("work");
    refused(&scratch.path("work/tape"), &[("work", &work)], "work");
    assert!(!scratch.path("work/tape").exists());
    accepted(&scratch.path("tapes/tape"), &[("work", &work)]);
    for owned in ["work", "work/child"] {
        assert_eq!(
            RecorderDirectory::pin(&scratch.path(owned))
                .unwrap()
                .check(&[("work", &work)])
                .unwrap_err(),
            RunRefusal::RecorderOverlap {
                mount: "work".into()
            }
        );
    }
    RecorderDirectory::pin(&scratch.path("tapes"))
        .unwrap()
        .check(&[("work", &work)])
        .unwrap();
    let child = scratch.directory("work/child");
    fs::write(scratch.path("work/tape"), b"untouched").unwrap();
    assert_eq!(
        RecorderDirectory::pin(&scratch.path("work"))
            .unwrap()
            .check(&[("child", &child)])
            .unwrap_err(),
        RunRefusal::RecorderOverlap {
            mount: "child".into()
        }
    );
    RecorderDirectory::pin(&scratch.path("tapes"))
        .unwrap()
        .check(&[("child", &child)])
        .unwrap();
    accepted(&scratch.path("work/tape"), &[("child", &child)]);
    accepted(&scratch.path("tape-next-to-work"), &[("work", &work)]);
    accepted(&scratch.path("tapes/child-tape"), &[("child", &child)]);
}

#[test]
fn symlink_aliases_and_parent_steps_use_physical_ancestry() {
    let scratch = Scratch::new("aliases");
    symlink(scratch.path("work"), scratch.path("alias")).unwrap();
    symlink(scratch.path("tapes"), scratch.path("outside")).unwrap();
    let work = scratch.directory("work");
    let alias = scratch.directory("alias");
    for (path, grant) in [
        ("alias/tape", &work),
        ("work/child/../tape", &work),
        ("work/tape", &alias),
        ("alias/child/../tape", &alias),
    ] {
        refused(&scratch.path(path), &[("work", grant)], "work");
        accepted(&scratch.path("outside/tape"), &[("work", grant)]);
    }
    assert!(!scratch.path("work/tape").exists());
    let child = scratch.directory("alias/child");
    assert_eq!(
        RecorderDirectory::pin(&scratch.path("alias"))
            .unwrap()
            .check(&[("child", &child)])
            .unwrap_err(),
        RunRefusal::RecorderOverlap {
            mount: "child".into()
        }
    );
    RecorderDirectory::pin(&scratch.path("outside"))
        .unwrap()
        .check(&[("child", &child)])
        .unwrap();
    accepted(&scratch.path("outside/tape"), &[("child", &child)]);
    fs::write(scratch.path("work/data"), b"untouched").unwrap();
    fs::write(scratch.path("tapes/data"), b"old").unwrap();
    symlink(scratch.path("work/data"), scratch.path("tapes/link")).unwrap();
    symlink(scratch.path("tapes/data"), scratch.path("other/link")).unwrap();
    refused(&scratch.path("tapes/link"), &[("work", &work)], "work");
    assert_eq!(fs::read(scratch.path("work/data")).unwrap(), b"untouched");
    accepted(&scratch.path("other/link"), &[("work", &work)]);
}

#[test]
fn every_mount_is_checked_even_when_first_mount_is_disjoint() {
    let scratch = Scratch::new("mounts");
    let work = scratch.directory("work");
    let other = scratch.directory("other");
    let grants = [("first", &other), ("second", &work)];
    refused(&scratch.path("work/tape"), &grants, "second");
    accepted(&scratch.path("tapes/tape"), &grants);
}

#[test]
fn replaced_path_designators_do_not_replace_the_granted_or_recorder_descriptors() {
    let scratch = Scratch::new("pins");
    let work = scratch.directory("work");
    let new = RecorderDestination::pin(&scratch.path("tapes/tape")).unwrap();
    fs::rename(scratch.path("work"), scratch.path("moved-work")).unwrap();
    symlink(scratch.path("tapes"), scratch.path("work")).unwrap();
    fs::rename(scratch.path("tapes"), scratch.path("moved-tapes")).unwrap();
    symlink(scratch.path("moved-work"), scratch.path("tapes")).unwrap();
    drop(new.open(&[("work", &work)]).unwrap());
    assert!(scratch.path("moved-tapes/tape").exists());
    assert!(!scratch.path("moved-work/tape").exists());
    refused(&scratch.path("moved-work/tape"), &[("work", &work)], "work");
    fs::write(scratch.path("moved-tapes/data"), b"old").unwrap();
    let existing = RecorderDestination::pin(&scratch.path("moved-tapes/data")).unwrap();
    fs::rename(
        scratch.path("moved-tapes/data"),
        scratch.path("moved-tapes/pinned"),
    )
    .unwrap();
    fs::write(scratch.path("moved-work/data"), b"untouched").unwrap();
    symlink(
        scratch.path("moved-work/data"),
        scratch.path("moved-tapes/data"),
    )
    .unwrap();
    drop(existing.open(&[("work", &work)]).unwrap());
    assert_eq!(fs::read(scratch.path("moved-tapes/pinned")).unwrap(), b"");
    assert_eq!(
        fs::read(scratch.path("moved-work/data")).unwrap(),
        b"untouched"
    );
}

#[test]
fn raced_final_symlink_and_shared_inodes_are_never_truncated() {
    let scratch = Scratch::new("leaf");
    let work = scratch.directory("work");
    fs::write(scratch.path("work/data"), b"untouched").unwrap();
    let recorder = RecorderDestination::pin(&scratch.path("tapes/raced")).unwrap();
    symlink(scratch.path("work/data"), scratch.path("tapes/raced")).unwrap();
    assert!(recorder.open(&[("work", &work)]).is_err());
    fs::hard_link(scratch.path("work/data"), scratch.path("tapes/hard-link")).unwrap();
    refused(&scratch.path("tapes/hard-link"), &[("work", &work)], "work");
    assert_eq!(fs::read(scratch.path("work/data")).unwrap(), b"untouched");
    accepted(&scratch.path("tapes/isolated"), &[("work", &work)]);
    fs::hard_link(
        scratch.path("tapes/isolated"),
        scratch.path("other/hard-link"),
    )
    .unwrap();
    accepted(&scratch.path("other/hard-link"), &[("work", &work)]);
}

struct Host {
    destination: PathBuf,
    pins: Vec<(String, PinnedDirectory)>,
    recorder: Option<File>,
}
impl HostAuthority for Host {
    fn authorize(&mut self, record: &RunConfig) -> Result<(), RunRefusal> {
        // Hand-built host policy grants directories only; unsupported providers
        // are refused instead of manufacturing broader authority.
        let io = record.io.as_ref().unwrap();
        assert!(
            io.stdout.is_none()
                && io.tasks.is_none()
                && io.net.is_none()
                && io.random.is_none()
                && io.monotonic_clock.is_none()
                && io.wall_clock.is_none()
        );
        for mount in &io.dirs {
            self.pins.push((
                mount.path.clone(),
                PinnedDirectory::open(Path::new(&mount.host)).map_err(|_| {
                    RunRefusal::Unauthorized {
                        field: mount.path.clone(),
                    }
                })?,
            ));
        }
        let grants: Vec<_> = self.pins.iter().map(|(m, d)| (m.as_str(), d)).collect();
        self.recorder = Some(RecorderDestination::pin(&self.destination)?.open(&grants)?);
        Ok(())
    }
    fn revocation(&mut self, reference: &str) -> Result<RevocationEvidence, RunRefusal> {
        Err(RunRefusal::MissingEvidence {
            reference: reference.into(),
        })
    }
}
#[test]
fn validated_host_admission_reports_recorder_overlap_before_entry_for_read_only_grants() {
    let scratch = Scratch::new("admission");
    let root = scratch.path("work");
    let json = format!(
        r#"{{"schema":1,"program":"{}","entry":"app@1/main","launch":"main",
        "inputs":{{"hash_seed":0,"random_seed":0,"test_seed":0,"scheduler":null}},
        "io":{{"stdout":null,"tasks":null,"monotonic_clock":null,"wall_clock":null,
        "random":null,"dirs":[{{"path":"work","host":"{}","access":"read"}}],"net":null}},
        "args":null,"env":null,"unsafe":false,"admission":null}}"#,
        "00".repeat(32),
        root.display()
    );
    let run = RunConfig::validate(json.as_bytes()).unwrap();
    for (tape, overlap) in [("work/tape", true), ("tapes/tape", false)] {
        let mut host = Host {
            destination: scratch.path(tape),
            pins: vec![],
            recorder: None,
        };
        let mut entered = false;
        let facts = hirsel_run_config::ProgramFacts {
            program: run.program(),
            entry: run.record().entry.clone(),
            launch: run.record().launch,
            roots: vec![],
            code: hirsel_run_config::CodeReach::Safe,
            fueled: true,
        };
        let result = run
            .clone()
            .bind(&facts)
            .unwrap()
            .admit(&mut host, 0)
            .map(|_| {
                entered = true;
            });
        assert_eq!(entered, !overlap);
        if overlap {
            assert_eq!(
                result.unwrap_err(),
                RunRefusal::RecorderOverlap {
                    mount: "work".into()
                }
            );
            assert!(!scratch.path(tape).exists());
        } else {
            result.unwrap();
        }
    }
    let io = RealIo::grant_pinned_with_output(scratch.directory("work"), std::io::sink());
    assert_eq!(
        io.prepare_recording_with_files(&scratch.path("work/tape"), &[])
            .unwrap_err(),
        RunRefusal::RecorderOverlap { mount: "".into() }
    );
    drop(
        io.prepare_recording_with_files(&scratch.path("tapes/real-tape"), &[])
            .unwrap(),
    );
}

#[test]
fn bind_mount_probe() {
    let Ok(root) = std::env::var("HIRSEL_RECORDER_BIND_PROBE") else {
        return;
    };
    let root = PathBuf::from(root);
    let source =
        std::ffi::CString::new(root.join("work/child").as_os_str().as_encoded_bytes()).unwrap();
    let target = std::ffi::CString::new(root.join("alias").as_os_str().as_encoded_bytes()).unwrap();
    // Runs in a private user/mount namespace created by the parent test.
    assert_eq!(
        unsafe {
            libc::mount(
                source.as_ptr(),
                target.as_ptr(),
                std::ptr::null(),
                libc::MS_BIND,
                std::ptr::null(),
            )
        },
        0,
        "{}",
        std::io::Error::last_os_error()
    );
    let work = PinnedDirectory::open(&root.join("work")).unwrap();
    refused(&root.join("alias/tape"), &[("work", &work)], "work");
    accepted(&root.join("tapes/tape"), &[("work", &work)]);
    let child = PinnedDirectory::open(&root.join("alias")).unwrap();
    assert_eq!(
        RecorderDirectory::pin(&root.join("work"))
            .unwrap()
            .check(&[("child", &child)])
            .unwrap_err(),
        RunRefusal::RecorderOverlap {
            mount: "child".into()
        }
    );
    RecorderDirectory::pin(&root.join("tapes"))
        .unwrap()
        .check(&[("child", &child)])
        .unwrap();
    accepted(&root.join("tapes/tape"), &[("child", &child)]);
    // Storage outside the grant can also be exposed by a bind mounted below it.
    let target =
        std::ffi::CString::new(root.join("work/exposed").as_os_str().as_encoded_bytes()).unwrap();
    let source = std::ffi::CString::new(root.join("tapes").as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(
        unsafe {
            libc::mount(
                source.as_ptr(),
                target.as_ptr(),
                std::ptr::null(),
                libc::MS_BIND,
                std::ptr::null(),
            )
        },
        0
    );
    refused(&root.join("tapes/tape"), &[("work", &work)], "work");
    accepted(&root.join("other/tape"), &[("work", &work)]);
    // A file bind mount must compare its endpoint inode too.
    fs::write(root.join("work/data"), b"untouched").unwrap();
    fs::write(root.join("other/bound-file"), b"old").unwrap();
    let source =
        std::ffi::CString::new(root.join("work/data").as_os_str().as_encoded_bytes()).unwrap();
    let target =
        std::ffi::CString::new(root.join("other/bound-file").as_os_str().as_encoded_bytes())
            .unwrap();
    assert_eq!(
        unsafe {
            libc::mount(
                source.as_ptr(),
                target.as_ptr(),
                std::ptr::null(),
                libc::MS_BIND,
                std::ptr::null(),
            )
        },
        0
    );
    refused(&root.join("other/bound-file"), &[("work", &work)], "work");
    assert_eq!(fs::read(root.join("work/data")).unwrap(), b"untouched");
    fs::write(root.join("other/source-file"), b"outside").unwrap();
    fs::write(root.join("work/exposed-file"), b"old").unwrap();
    let source = std::ffi::CString::new(
        root.join("other/source-file")
            .as_os_str()
            .as_encoded_bytes(),
    )
    .unwrap();
    let target = std::ffi::CString::new(
        root.join("work/exposed-file")
            .as_os_str()
            .as_encoded_bytes(),
    )
    .unwrap();
    assert_eq!(
        unsafe {
            libc::mount(
                source.as_ptr(),
                target.as_ptr(),
                std::ptr::null(),
                libc::MS_BIND,
                std::ptr::null(),
            )
        },
        0
    );
    refused(&root.join("other/source-file"), &[("work", &work)], "work");
    assert_eq!(
        fs::read(root.join("other/source-file")).unwrap(),
        b"outside"
    );
    // Two bind views of one directory may have different mounts at the same
    // child name. Both must be scanned, regardless of read_dir ordering.
    fs::create_dir(root.join("shared")).unwrap();
    fs::write(root.join("shared/slot"), b"placeholder").unwrap();
    for view in ["view-a", "view-b"] {
        fs::create_dir(root.join("work").join(view)).unwrap();
        fs::write(root.join("other").join(view), b"isolated").unwrap();
        let source =
            std::ffi::CString::new(root.join("shared").as_os_str().as_encoded_bytes()).unwrap();
        let target =
            std::ffi::CString::new(root.join("work").join(view).as_os_str().as_encoded_bytes())
                .unwrap();
        assert_eq!(
            unsafe {
                libc::mount(
                    source.as_ptr(),
                    target.as_ptr(),
                    std::ptr::null(),
                    libc::MS_BIND,
                    std::ptr::null(),
                )
            },
            0
        );
        let source =
            std::ffi::CString::new(root.join("other").join(view).as_os_str().as_encoded_bytes())
                .unwrap();
        let target = std::ffi::CString::new(
            root.join("work")
                .join(view)
                .join("slot")
                .as_os_str()
                .as_encoded_bytes(),
        )
        .unwrap();
        assert_eq!(
            unsafe {
                libc::mount(
                    source.as_ptr(),
                    target.as_ptr(),
                    std::ptr::null(),
                    libc::MS_BIND,
                    std::ptr::null(),
                )
            },
            0
        );
    }
    for view in ["view-a", "view-b"] {
        refused(&root.join("other").join(view), &[("work", &work)], "work");
        assert_eq!(
            fs::read(root.join("other").join(view)).unwrap(),
            b"isolated"
        );
    }
    accepted(&root.join("other/isolated-after-views"), &[("work", &work)]);
}
#[test]
fn bind_mount_aliases_are_checked_when_private_mount_namespaces_are_available() {
    use std::process::Command;
    let available = Command::new("unshare")
        .args(["--user", "--map-root-user", "--mount", "true"])
        .output();
    if !available.is_ok_and(|output| output.status.success()) {
        eprintln!("bind mount coverage unavailable: private user/mount namespace denied");
        return;
    }
    let scratch = Scratch::new("bind space");
    fs::create_dir(scratch.path("alias")).unwrap();
    fs::create_dir(scratch.path("work/exposed")).unwrap();
    let output = Command::new("unshare")
        .args(["--user", "--map-root-user", "--mount"])
        .arg(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "recorder_overlap::bind_mount_probe",
            "--nocapture",
        ])
        .env("HIRSEL_RECORDER_BIND_PROBE", &scratch.0)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
}

unsafe extern "C" fn entry(
    _: *mut hirsel_rt::RunCtx,
    _: *mut u8,
    out: *mut u8,
    _: *const u64,
    _: *const u64,
) -> u8 {
    println!("recorder-entry-reached");
    if std::env::var("HIRSEL_RECORDER_PROBE").as_deref() == Ok("replace") {
        let destination = PathBuf::from(std::env::var_os("RECORDER_DESTINATION").unwrap());
        fs::rename(&destination, destination.with_extension("pinned")).unwrap();
        symlink(
            std::env::var_os("RECORDER_GRANTED_DATA").unwrap(),
            &destination,
        )
        .unwrap();
    }
    unsafe { out.cast::<u32>().write(0) };
    0
}
#[test]
fn launcher_probe() {
    if std::env::var_os("HIRSEL_RECORDER_PROBE").is_none() {
        return;
    }
    let config = hirsel_rt::launch::Config::default();
    let exit =
        unsafe { hirsel_rt::launch::hirsel_rt_v2_launch(entry, 4, std::ptr::null(), 0, &config) };
    std::process::exit(exit);
}
fn probe(root: &Path, destination: &Path, field: &str, replace: bool) -> std::process::Output {
    std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "recorder_overlap::launcher_probe", "--nocapture"])
        .env_clear()
        .env(
            "HIRSEL_RECORDER_PROBE",
            if replace { "replace" } else { "ordinary" },
        )
        .env("HIRSEL_RT_ROOT", root)
        .env(field, destination)
        .env("RECORDER_DESTINATION", destination)
        .env("RECORDER_GRANTED_DATA", root.join("data"))
        .output()
        .unwrap()
}
fn tool_outputs() -> Vec<&'static str> {
    let fields = vec!["HIRSEL_RT_TAPE", "HIRSEL_RT_OBSERVATION"];
    #[cfg(feature = "measurement")]
    let fields = {
        let mut fields = fields;
        fields.push("HIRSEL_RT_MEASUREMENT");
        fields
    };
    fields
}
#[test]
fn every_launcher_recorder_refuses_inside_grant_before_entry_and_accepts_sibling_storage() {
    let scratch = Scratch::new("launcher");
    for field in tool_outputs() {
        let refused = scratch.path(&format!("work/{field}"));
        fs::write(&refused, b"untouched").unwrap();
        let output = probe(&scratch.path("work"), &refused, field, false);
        assert_eq!(output.status.code(), Some(3), "{output:?}");
        assert!(!String::from_utf8_lossy(&output.stdout).contains("recorder-entry-reached"));
        assert!(String::from_utf8_lossy(&output.stderr).contains(r#""kind":"recorder_overlap""#));
        assert_eq!(fs::read(&refused).unwrap(), b"untouched");
        // Same source entry and grant; only the destination differs. This file
        // is beside the grant in a shared parent, not inside the grant.
        let accepted = scratch.path(field);
        let output = probe(&scratch.path("work"), &accepted, field, false);
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        assert!(String::from_utf8_lossy(&output.stdout).contains("recorder-entry-reached"));
        assert!(!fs::read(&accepted).unwrap().is_empty());
    }
}
#[test]
fn launcher_outputs_keep_the_admitted_inode_after_their_names_change_during_entry() {
    let scratch = Scratch::new("launcher-pins");
    fs::write(scratch.path("work/data"), b"untouched").unwrap();
    for field in tool_outputs() {
        let destination = scratch.path(&format!("tapes/{field}"));
        let output = probe(&scratch.path("work"), &destination, field, true);
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        assert!(
            !fs::read(destination.with_extension("pinned"))
                .unwrap()
                .is_empty()
        );
        assert_eq!(fs::read(scratch.path("work/data")).unwrap(), b"untouched");
    }
}

#[test]
fn inherited_output_files_cannot_alias_any_recorder_but_disjoint_sinks_are_accepted() {
    use std::process::{Command, Stdio};
    let scratch = Scratch::new("stdio");
    for field in tool_outputs() {
        for descriptor in ["stdout", "stderr"] {
            for collide in [true, false] {
                let stem = format!("{field}-{descriptor}-{collide}");
                let recorder = scratch.path(&format!("tapes/{stem}"));
                fs::write(&recorder, b"sentinel").unwrap();
                let sink = if collide {
                    recorder.clone()
                } else {
                    scratch.path(&format!("other/{stem}"))
                };
                if !collide {
                    fs::write(&sink, b"").unwrap();
                }
                let output = File::options().write(true).open(&sink).unwrap();
                let mut command = Command::new(std::env::current_exe().unwrap());
                command
                    .args(["--exact", "recorder_overlap::launcher_probe", "--nocapture"])
                    .env_clear()
                    .env("HIRSEL_RECORDER_PROBE", "ordinary")
                    .env("HIRSEL_RT_ROOT", scratch.path("work"))
                    .env(field, &recorder);
                if descriptor == "stdout" {
                    command.stdout(Stdio::from(output));
                } else {
                    command.stderr(Stdio::from(output));
                }
                let result = command.output().unwrap();
                assert_eq!(
                    result.status.code(),
                    Some(if collide { 3 } else { 0 }),
                    "{result:?}"
                );
                if collide {
                    let diagnostic = if descriptor == "stderr" {
                        fs::read(&sink).unwrap()
                    } else {
                        result.stderr.clone()
                    };
                    let diagnostic = String::from_utf8_lossy(&diagnostic);
                    assert!(
                        diagnostic.contains(r#""kind":"recorder_overlap""#),
                        "{diagnostic}"
                    );
                    assert!(
                        diagnostic.contains(&format!(r#""mount":"{descriptor}""#)),
                        "{diagnostic}"
                    );
                    let stdout = if descriptor == "stdout" {
                        fs::read(&sink).unwrap()
                    } else {
                        result.stdout.clone()
                    };
                    assert!(!String::from_utf8_lossy(&stdout).contains("recorder-entry-reached"));
                } else {
                    assert!(!fs::read(&recorder).unwrap().is_empty());
                }
            }
        }
    }
}
