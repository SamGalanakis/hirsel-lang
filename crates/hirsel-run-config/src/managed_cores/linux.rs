//! Cgroup-v2 discovery follows Go's membership -> mount -> cpu.max structure
//! (internal/runtime/cgroup/cgroup_linux.go:201-237, ebe2feefc400). Unlike Go's
//! leaf-only reader, this walks every visible ancestor, as required by DET-3.
use super::{CpuSet, HostLimits, Quota};

mod root_mount;
use crate::{RunRefusal, invalid};
use root_mount::root_mount;
use std::{
    fs, io,
    io::BufRead,
    path::{Component, Path, PathBuf},
};

pub struct LinuxLimits {
    proc_root: PathBuf,
    admission_affinity: CpuSet,
    probe_root_mount: bool,
}
impl LinuxLimits {
    /// Call on the admission thread before any pinning. Rechecks never inspect a
    /// worker's affinity. Replay must never construct this host-only reader.
    pub fn capture() -> Result<Self, RunRefusal> {
        let mut reader = Self::from_proc(Path::new("/proc"))?;
        reader.probe_root_mount = true;
        Ok(reader)
    }
    /// Alternate proc root for host adapters and deterministic filesystem fixtures.
    pub fn from_proc(proc_root: &Path) -> Result<Self, RunRefusal> {
        let status = read(&proc_root.join("thread-self/status"))?;
        let affinity = status
            .lines()
            .find_map(|line| line.strip_prefix("Cpus_allowed_list:"))
            .ok_or_else(|| invalid("affinity", "Cpus_allowed_list missing"))?;
        Ok(Self {
            proc_root: proc_root.into(),
            probe_root_mount: false,
            admission_affinity: CpuSet::parse(affinity)?,
        })
    }
    pub fn read(&self) -> Result<HostLimits, RunRefusal> {
        let mut limits = HostLimits {
            admission_affinity: self.admission_affinity.clone(),
            effective_cpuset: self.admission_affinity.clone(),
            quota: None,
            throttled_usec: 0,
        };
        // Rediscover membership on each check: a manager can move the run.
        let membership = read(&self.proc_root.join("self/cgroup"))?;
        let unified = membership.lines().find_map(|line| line.strip_prefix("0::"));
        let Some(group) = unified else {
            if !membership.trim().is_empty() {
                return Err(invalid("cgroup", "managed cores requires cgroup v2"));
            }
            return Ok(limits);
        };
        let group = absolute(group)?;
        // A kernel-proved filesystem root exposes every ancestor. Otherwise
        // retain full mount discovery, including delegated and escaped mounts.
        let (leaf, root) = match self.probe_root_mount.then(root_mount).flatten() {
            Some(root) => (root.join(group.strip_prefix("/").unwrap()), root),
            None => self.discover_mount(&group)?,
        };
        if !leaf.is_dir() {
            return Err(invalid(
                "cgroup",
                "membership disappeared during resolution",
            ));
        }
        let mut cpuset_found = false;
        for directory in leaf.ancestors() {
            if let Some(text) = optional(&directory.join("cpu.max"))?
                && let Some(q) = Quota::parse(&text)?
            {
                limits.quota = Some(limits.quota.map_or(q, |old| old.minimum(q)));
            }
            if !cpuset_found && let Some(text) = optional(&directory.join("cpuset.cpus.effective"))?
            {
                if text.trim().is_empty() {
                    return Err(invalid("cpuset", "empty effective cpuset"));
                }
                limits.effective_cpuset = CpuSet::parse(&text)?;
                cpuset_found = true;
            }
            if directory == root {
                break;
            }
        }
        if let Some(stat) = optional(&leaf.join("cpu.stat"))? {
            for line in stat.lines() {
                if let Some(value) = line.strip_prefix("throttled_usec ") {
                    limits.throttled_usec = value
                        .parse()
                        .map_err(|_| invalid("cpu.stat", "invalid throttled_usec"))?;
                }
            }
        }
        Ok(limits)
    }
    fn discover_mount(&self, group: &Path) -> Result<(PathBuf, PathBuf), RunRefusal> {
        let mountinfo = self.proc_root.join("self/mountinfo");
        let failed =
            |e: io::Error| invalid("host limits", &format!("{}: {e}", mountinfo.display()));
        // Reuse one line, as in Rust BufRead::read_line. A full mount table can
        // cross libc's mmap threshold and change later runtime slab backing.
        let mut mounts = io::BufReader::new(fs::File::open(&mountinfo).map_err(failed)?);
        let mut line = String::new();
        let mut candidates = Vec::new();
        loop {
            line.clear();
            if mounts.read_line(&mut line).map_err(failed)? == 0 {
                break;
            }
            let Some((before, after)) = line.split_once(" - ") else {
                continue;
            };
            if after.split_whitespace().next() != Some("cgroup2") {
                continue;
            }
            let fields: Vec<_> = before.split_whitespace().collect();
            if fields.len() < 6 {
                return Err(invalid("mountinfo", "short cgroup2 mount"));
            }
            let root = absolute(&unescape(fields[3])?)?;
            let mount = absolute(&unescape(fields[4])?)?;
            if let Ok(relative) = group.strip_prefix(&root) {
                candidates.push((root.components().count(), mount.join(relative), mount));
            }
        }
        // Prefer the mount exposing the most ancestors; a delegated bind mount
        // must not hide a quota visible through the namespace's root mount.
        candidates.sort_by_key(|entry| entry.0);
        let (_, leaf, root) = candidates
            .into_iter()
            .next()
            .ok_or_else(|| invalid("cgroup", "no visible cgroup2 mount for membership"))?;
        Ok((leaf, root))
    }
}
fn absolute(text: &str) -> Result<PathBuf, RunRefusal> {
    let path = Path::new(text);
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(invalid(
            "cgroup",
            "expected an absolute path without traversal",
        ));
    }
    Ok(path.into())
}
fn unescape(text: &str) -> Result<String, RunRefusal> {
    // proc mountinfo encodes space, tab, newline and backslash as octal.
    let mut out = Vec::new();
    let mut bytes = text.as_bytes().iter().copied();
    while let Some(byte) = bytes.next() {
        out.push(if byte == b'\\' {
            match (bytes.next(), bytes.next(), bytes.next()) {
                (Some(b'0'), Some(b'4'), Some(b'0')) => b' ',
                (Some(b'0'), Some(b'1'), Some(b'1')) => b'\t',
                (Some(b'0'), Some(b'1'), Some(b'2')) => b'\n',
                (Some(b'1'), Some(b'3'), Some(b'4')) => b'\\',
                _ => return Err(invalid("mountinfo", "invalid path escape")),
            }
        } else {
            byte
        });
    }
    String::from_utf8(out).map_err(|_| invalid("mountinfo", "non-UTF-8 cgroup path"))
}
fn read(path: &Path) -> Result<String, RunRefusal> {
    fs::read_to_string(path)
        .map_err(|e| invalid("host limits", &format!("{}: {e}", path.display())))
}
fn optional(path: &Path) -> Result<Option<String>, RunRefusal> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(invalid("host limits", &format!("{}: {e}", path.display()))),
    }
}
