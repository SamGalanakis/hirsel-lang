//! Cgroup-v2 discovery follows Go's membership -> mount -> cpu.max structure
//! (internal/runtime/cgroup/cgroup_linux.go:201-237, ebe2feefc400). Unlike Go's
//! leaf-only reader, this walks every visible ancestor, as required by DET-3.
use super::{CpuSet, HostLimits, Quota};

mod directory;
mod root_mount;
use directory::Directory;
use root_mount::root_mount;

use crate::{RunRefusal, invalid};
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
        Ok(Self {
            proc_root: PathBuf::from("/proc"),
            admission_affinity: affinity()?,
            probe_root_mount: true,
        })
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
            admission_affinity: CpuSet::parse(affinity)?,
            probe_root_mount: false,
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
        // A complete cgroup2 filesystem root exposes every ancestor. Delegated
        // roots retain mount discovery, including escaped mount paths.
        let (leaf, root, opened) = match self.probe_root_mount.then(root_mount).flatten() {
            Some((root, directory)) => (
                root.join(group.strip_prefix("/").unwrap()),
                root,
                Some(directory),
            ),
            None => {
                let (leaf, root) = self.discover_mount(&group)?;
                (leaf, root, None)
            }
        };
        if !leaf.is_dir() {
            return Err(invalid(
                "cgroup",
                "membership disappeared during resolution",
            ));
        }
        let levels = Directory::walk(&root, &leaf, opened)?;
        let mut cpuset_found = false;
        for directory in levels.iter().rev() {
            if let Some(text) = directory.read(c"cpu.max")?
                && let Some(q) = Quota::parse(&text)?
            {
                limits.quota = Some(limits.quota.map_or(q, |old| old.minimum(q)));
            }
            if !cpuset_found && let Some(text) = directory.read(c"cpuset.cpus.effective")? {
                if text.trim().is_empty() {
                    return Err(invalid("cpuset", "empty effective cpuset"));
                }
                limits.effective_cpuset = CpuSet::parse(&text)?;
                cpuset_found = true;
            }
        }
        if let Some(stat) = levels.last().unwrap().read(c"cpu.stat")? {
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

/// Go 1.25 runtime/os_linux.go:106-127 obtains affinity directly from the
/// kernel. Grow the mask on EINVAL, as glibc's CPU_ALLOC interface permits,
/// so machines beyond CPU_SETSIZE retain their complete admission view.
fn affinity() -> Result<CpuSet, RunRefusal> {
    let mut mask = vec![0 as libc::c_ulong; 16];
    loop {
        // SAFETY: mask is aligned and writable for the supplied byte length.
        // Linux accepts a dynamically sized CPU mask, independently of the
        // size of libc's cpu_set_t. PID zero selects the calling thread.
        if unsafe {
            libc::sched_getaffinity(
                0,
                std::mem::size_of_val(mask.as_slice()),
                mask.as_mut_ptr().cast(),
            )
        } == 0
        {
            let mut ranges: Vec<(u32, u32)> = Vec::new();
            for (word, &bits) in mask.iter().enumerate() {
                let mut bits = bits;
                while bits != 0 {
                    let cpu = word * libc::c_ulong::BITS as usize + bits.trailing_zeros() as usize;
                    let cpu = u32::try_from(cpu)
                        .map_err(|_| invalid("affinity", "CPU index exceeds u32"))?;
                    match ranges.last_mut() {
                        Some((_, hi)) if hi.checked_add(1) == Some(cpu) => *hi = cpu,
                        _ => ranges.push((cpu, cpu)),
                    }
                    bits &= bits - 1;
                }
            }
            return Ok(CpuSet(ranges));
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::EINVAL) {
            return Err(invalid("affinity", &error.to_string()));
        }
        let length = mask
            .len()
            .checked_mul(2)
            .ok_or_else(|| invalid("affinity", "CPU mask size overflow"))?;
        mask.resize(length, 0);
    }
}
