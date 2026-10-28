// Ownership is the private job's effective cpuset. Exclusivity is a separate
// host property: every branch outside the job's ancestry must be disjoint.
#[derive(Debug)]
struct CpuOwnership {
    cpus: BTreeSet<usize>,
    group: String,
    exclusivity: Result<(), String>,
}

fn parse_cpu_list(text: &str) -> Result<BTreeSet<usize>, String> {
    let mut cpus = BTreeSet::new();
    for field in text.trim().split(',').filter(|s| !s.is_empty()) {
        let (first, last) = field.split_once('-').unwrap_or((field, field));
        let first = first.parse::<usize>().map_err(|e| e.to_string())?;
        let last = last.parse::<usize>().map_err(|e| e.to_string())?;
        if first > last || last >= libc::CPU_SETSIZE as usize {
            return Err(format!("invalid CPU range: {field}"));
        }
        cpus.extend(first..=last);
    }
    Ok(cpus)
}

fn disjoint_cgroup_branches(
    root: &std::path::Path,
    leaf: &std::path::Path,
    cpus: &BTreeSet<usize>,
) -> Result<(), String> {
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        if directory == leaf {
            continue;
        }
        if !leaf.starts_with(&directory) {
            let effective = fs::read_to_string(directory.join("cpuset.cpus.effective"))
                .map_err(|e| format!("cannot check {}: {e}", directory.display()))?;
            let other = parse_cpu_list(&effective)?;
            let intersection: Vec<_> = other.intersection(cpus).copied().collect();
            if !intersection.is_empty() {
                return Err(format!(
                    "host-blocked: not exclusive: {} intersects owned CPUs {intersection:?}",
                    directory.display()
                ));
            }
            // A child's effective set is a subset of this disjoint parent.
            continue;
        }
        // Ancestor cpusets necessarily include ours. Processes resident in an
        // ancestor can still run here, unlike its disjoint child branches.
        let members = fs::read_to_string(directory.join("cgroup.procs"))
            .map_err(|e| format!("cannot check {}: {e}", directory.display()))?;
        for pid in members.split_whitespace() {
            let status = match fs::read_to_string(format!("/proc/{pid}/status")) {
                Ok(status) => status,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(format!("cannot check ancestor PID {pid}: {error}")),
            };
            // Kernel threads live in the hierarchy root even under a systemd
            // fence. Their preemptions still invalidate switch-free samples.
            if !status.lines().any(|line| line == "Kthread:\t1") {
                return Err(format!(
                    "host-blocked: not exclusive: {} has resident userspace PID {pid}",
                    directory.display()
                ));
            }
        }
        for entry in fs::read_dir(&directory).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
                pending.push(entry.path());
            }
        }
    }
    Ok(())
}

fn owned_cpus(affinity: &[usize]) -> Result<CpuOwnership, String> {
    let membership = fs::read_to_string("/proc/self/cgroup").map_err(|e| e.to_string())?;
    let group = membership
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .ok_or("no unified cgroup membership")?;
    if group == "/" || !group.starts_with('/') || group.split('/').any(|p| p == "..") {
        return Err("no visible private cgroup".into());
    }
    let root = std::path::Path::new("/sys/fs/cgroup");
    let leaf = root.join(group.trim_start_matches('/'));
    let read = |path: &std::path::Path| {
        fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
    };
    let members = read(&leaf.join("cgroup.procs"))?;
    let stat = read(&leaf.join("cgroup.stat"))?;
    if !private_test_processes(&members, &test_process_chain()?)
        || !stat.lines().any(|line| line == "nr_descendants 0")
    {
        return Err("cgroup contains processes or descendants outside this test job".into());
    }
    let cpus = parse_cpu_list(&read(&leaf.join("cpuset.cpus.effective"))?)?;
    let admitted: BTreeSet<_> = affinity.iter().copied().collect();
    if cpus != admitted {
        return Err(format!(
            "private cpuset {cpus:?} differs from affinity {admitted:?}"
        ));
    }
    for directory in leaf.ancestors() {
        let path = directory.join("cpu.max");
        if path.exists() {
            let text = read(&path)?;
            let fields: Vec<_> = text.split_whitespace().collect();
            if fields.len() != 2 {
                return Err(format!("invalid quota: {text:?}"));
            }
            if fields[0] != "max" {
                let amount = fields[0].parse::<usize>().map_err(|e| e.to_string())?;
                let period = fields[1].parse::<usize>().map_err(|e| e.to_string())?;
                let quota = amount.checked_div(period).ok_or("zero quota period")?;
                if quota < cpus.len() {
                    return Err(format!(
                        "quota {quota} is below owned CPU count {}",
                        cpus.len()
                    ));
                }
            }
        }
        if directory == root {
            break;
        }
    }
    Ok(CpuOwnership {
        exclusivity: disjoint_cgroup_branches(root, &leaf, &cpus),
        cpus,
        group: group.to_owned(),
    })
}

fn exclusive_cpus(affinity: &[usize]) -> Result<BTreeSet<usize>, String> {
    let ownership = owned_cpus(affinity)?;
    ownership.exclusivity?;
    Ok(ownership.cpus)
}

fn test_process_chain() -> Result<BTreeSet<u32>, String> {
    let mut chain = BTreeSet::new();
    let mut pid = std::process::id();
    while pid != 0 && chain.insert(pid) {
        let status =
            fs::read_to_string(format!("/proc/{pid}/status")).map_err(|e| e.to_string())?;
        pid = status
            .lines()
            .find_map(|line| line.strip_prefix("PPid:"))
            .ok_or("missing process parent")?
            .trim()
            .parse::<u32>()
            .map_err(|e| e.to_string())?;
    }
    Ok(chain)
}

fn private_test_processes(members: &str, chain: &BTreeSet<u32>) -> bool {
    let members = members
        .split_whitespace()
        .map(str::parse::<u32>)
        .collect::<Result<BTreeSet<_>, _>>();
    members.is_ok_and(|members| members.contains(&std::process::id()) && members.is_subset(chain))
}

fn partition_cpus(
    partition: &str,
    exclusive: &str,
    effective: &str,
    affinity: &[usize],
    quota: usize,
) -> Result<BTreeSet<usize>, String> {
    if !matches!(partition.trim(), "root" | "isolated") {
        return Err(format!("not a valid cpuset partition: {partition:?}"));
    }
    let exclusive = parse_cpu_list(exclusive)?;
    let effective = parse_cpu_list(effective)?;
    let cpus: BTreeSet<_> = affinity
        .iter()
        .copied()
        .filter(|cpu| exclusive.contains(cpu) && effective.contains(cpu))
        .collect();
    // A quota is a ceiling, not an exclusive share. A smaller quota can
    // throttle the entire job even when its cpuset is exclusively held.
    if quota < cpus.len() {
        return Err(format!(
            "quota {quota} is below the exclusive CPU count {}",
            cpus.len()
        ));
    }
    Ok(cpus)
}

#[cfg(test)]
mod exclusive_cpu_tests {
    use super::*;

    fn cgroup_fixture() -> (tempfile::TempDir, std::path::PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let leaf = root.path().join("window/job");
        fs::create_dir_all(&leaf).unwrap();
        fs::write(root.path().join("cgroup.procs"), "").unwrap();
        fs::write(root.path().join("window/cgroup.procs"), "").unwrap();
        (root, leaf)
    }

    #[test]
    fn delegated_member_is_owned_but_intersecting_sibling_is_not_exclusive() {
        let (root, leaf) = cgroup_fixture();
        let sibling = root.path().join("shared");
        fs::create_dir(&sibling).unwrap();
        fs::write(sibling.join("cpuset.cpus.effective"), "0-31").unwrap();
        let cpus = parse_cpu_list("16-19").unwrap();
        let error = disjoint_cgroup_branches(root.path(), &leaf, &cpus).unwrap_err();
        assert!(error.contains("host-blocked: not exclusive"), "{error}");
        assert!(
            error.contains("shared") && error.contains("16, 17, 18, 19"),
            "{error}"
        );
    }

    #[test]
    fn external_fence_can_prove_exclusivity_without_partition_root() {
        let (root, leaf) = cgroup_fixture();
        for (name, cpus) in [("other", "0-7,16-31"), ("window/other-job", "12-15")] {
            let branch = root.path().join(name);
            fs::create_dir(&branch).unwrap();
            fs::write(branch.join("cpuset.cpus.effective"), cpus).unwrap();
        }
        assert_eq!(
            disjoint_cgroup_branches(root.path(), &leaf, &parse_cpu_list("8-11").unwrap()),
            Ok(())
        );
    }

    #[test]
    fn unreadable_branch_and_ancestor_residents_refuse_exclusivity() {
        let (root, leaf) = cgroup_fixture();
        fs::create_dir(root.path().join("unknown")).unwrap();
        let cpus = parse_cpu_list("8-11").unwrap();
        assert!(disjoint_cgroup_branches(root.path(), &leaf, &cpus).is_err());
        fs::remove_dir(root.path().join("unknown")).unwrap();
        fs::write(
            root.path().join("window/cgroup.procs"),
            std::process::id().to_string(),
        )
        .unwrap();
        let error = disjoint_cgroup_branches(root.path(), &leaf, &cpus).unwrap_err();
        assert!(error.contains("resident userspace PID"), "{error}");
    }

    #[test]
    fn affinity_and_invalid_or_shared_partitions_never_prove_ownership() {
        let affinity: Vec<_> = (0..16).collect();
        for partition in ["member", "root invalid (parent is not a partition root)"] {
            assert!(partition_cpus(partition, "0-15", "0-15", &affinity, 16).is_err());
        }
        assert!(
            partition_cpus("root", "", "0-15", &affinity, 16)
                .unwrap()
                .is_empty()
        );
        assert!(partition_cpus("isolated", "0-15", "0-15", &affinity, 15).is_err());
        assert!(partition_cpus("root", "15-0", "0-15", &affinity, 16).is_err());
    }

    #[test]
    fn private_partition_retains_sixteen_and_intersects_every_limit() {
        // Buck's reporter and launcher remain alive as ancestors of libtest.
        // They belong to this job; a sibling action does not.
        let chain = test_process_chain().unwrap();
        assert!(chain.contains(&std::process::id()));
        assert!(chain.contains(&(unsafe { libc::getppid() } as u32)));
        let members = chain
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(private_test_processes(&members, &chain));
        assert!(!private_test_processes(&format!("{members}\n0"), &chain));
        assert!(!private_test_processes("", &chain));
        let affinity: Vec<_> = (0..16).collect();
        assert_eq!(
            partition_cpus("root", "0-15", "0-15", &affinity, 16)
                .unwrap()
                .len(),
            16
        );
        assert_eq!(
            partition_cpus("isolated", "0-3,8-11", "2-15", &affinity[..10], 16).unwrap(),
            BTreeSet::from([2, 3, 8, 9])
        );
    }

    #[test]
    fn overlap_still_requires_every_exclusively_held_cpu() {
        let cpus: BTreeSet<_> = (0..16).collect();
        let simultaneous = (0..16)
            .map(|worker| format!("cpu 1 {worker} {worker} 10 20 5\n"))
            .collect::<String>();
        assert!(overlap(&simultaneous, 16, &cpus));
        let serial = (0..16)
            .map(|worker| {
                format!(
                    "cpu 1 {worker} {worker} {} {} 5\n",
                    worker * 10,
                    worker * 10 + 5
                )
            })
            .collect::<String>();
        assert!(!overlap(&serial, 16, &cpus));
        assert!(!overlap(&serial, 16, &BTreeSet::from([0, 1])));
        assert!(overlap(&serial, 16, &BTreeSet::new()));
        let outside = simultaneous.replace("cpu 1 15 15 ", "cpu 1 15 31 ");
        assert!(!overlap(&outside, 16, &cpus));
        // Merely touching endpoints is not a positive common interval.
        assert!(!overlap(
            "cpu 1 0 0 0 10 5\ncpu 1 1 1 10 20 5\n",
            2,
            &BTreeSet::from([0, 1])
        ));
    }
}
