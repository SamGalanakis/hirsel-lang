#[derive(Debug, Default)]
struct PhysicalThreads(BTreeMap<String, (String, String)>);

impl PhysicalThreads {
    fn capture(&mut self, pid: u32) {
        let Ok(entries) = fs::read_dir(format!("/proc/{pid}/task")) else {
            return;
        };
        for entry in entries.flatten() {
            let mut snapshot = String::new();
            for name in ["comm", "sched", "schedstat", "status"] {
                if let Ok(text) = fs::read_to_string(entry.path().join(name)) {
                    snapshot.push_str(&format!("{name}: {text}\n"));
                }
            }
            let tid = entry.file_name().to_string_lossy().into_owned();
            self.0
                .entry(tid)
                .and_modify(|(_, last)| last.clone_from(&snapshot))
                .or_insert_with(|| (snapshot.clone(), snapshot));
        }
    }
}

fn physical_host_snapshot(pid: u32, cpus: &[usize]) -> String {
    let mut snapshot = format!("executor pid={pid} affinity={cpus:?}\n");
    for path in [
        "/etc/hostname".to_owned(),
        "/proc/sys/kernel/osrelease".to_owned(),
        format!("/proc/{pid}/cgroup"),
        format!("/proc/{pid}/mountinfo"),
    ] {
        snapshot.push_str(&format!("{path}: {:?}\n", fs::read_to_string(&path)));
    }
    // The native child inherits this test's cgroup. Preserve the parent's
    // readable ancestor counters even after the child's proc directory exits.
    if let Ok(membership) = fs::read_to_string("/proc/self/cgroup") {
        if let Some(group) = membership.lines().find_map(|line| line.strip_prefix("0::")) {
            let leaf = std::path::Path::new("/sys/fs/cgroup").join(group.trim_start_matches('/'));
            for directory in leaf.ancestors() {
                for name in [
                    "cpu.max",
                    "cpu.stat",
                    "cpu.pressure",
                    "cpuset.cpus.effective",
                    "cpuset.cpus.exclusive.effective",
                ] {
                    let path = directory.join(name);
                    if let Ok(text) = fs::read_to_string(&path) {
                        snapshot.push_str(&format!("{}: {text}\n", path.display()));
                    }
                }
                if directory == std::path::Path::new("/sys/fs/cgroup") {
                    break;
                }
            }
        }
    }
    for cpu in cpus {
        for name in ["core_id", "physical_package_id", "thread_siblings_list"] {
            let path = format!("/sys/devices/system/cpu/cpu{cpu}/topology/{name}");
            snapshot.push_str(&format!("{path}: {:?}\n", fs::read_to_string(&path)));
        }
    }
    snapshot
}
