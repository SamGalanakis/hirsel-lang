//! Descriptor-pinned filesystem portion of HostAuthority::authorize.
//! These pins do not authorize providers or access rights. An adapter retains each
//! granted pin, checks every recorder against all of them, and uses the same
//! descriptors for execution. Tool destinations are outside semantic RunConfig.
use hirsel_run_schema::RunRefusal;
use std::collections::BTreeSet;
use std::ffi::{CString, OsStr};
use std::fs::File;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

type Identity = (u64, u64);
fn identity(file: &File) -> std::io::Result<Identity> {
    let m = file.metadata()?;
    Ok((m.dev(), m.ino()))
}
fn open_at(fd: RawFd, path: &OsStr, flags: i32) -> std::io::Result<File> {
    let path = CString::new(path.as_bytes())?;
    // SAFETY: borrowed NUL-terminated path; fd is live or AT_FDCWD. The
    // successful syscall transfers one owned descriptor to File.
    let raw = unsafe { libc::openat(fd, path.as_ptr(), flags | libc::O_CLOEXEC, 0o600) };
    if raw < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(unsafe { File::from_raw_fd(raw) })
    }
}
fn invalid(field: &str, error: impl std::fmt::Display) -> RunRefusal {
    RunRefusal::Invalid {
        field: field.into(),
        detail: error.to_string(),
    }
}

/// Pin inherited output before any recorder opens can reuse a closed fd 1/2.
/// Non-file sinks and closed descriptors cannot alias regular recorder storage.
pub(crate) fn inherited_output_pins() -> Result<Vec<(&'static str, File)>, RunRefusal> {
    let mut pins = vec![];
    for (name, fd) in [
        ("stdout", libc::STDOUT_FILENO),
        ("stderr", libc::STDERR_FILENO),
    ] {
        let duplicate = loop {
            // SAFETY: fcntl validates the raw inherited descriptor. A successful
            // duplicate is owned, close-on-exec, and never occupies fd 0/1/2.
            let raw = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
            if raw >= 0 {
                break Some(unsafe { File::from_raw_fd(raw) });
            }
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            if error.raw_os_error() == Some(libc::EBADF) {
                break None;
            }
            return Err(invalid("recorder.sinks", error));
        };
        if let Some(file) = duplicate
            && file
                .metadata()
                .map_err(|e| invalid("recorder.sinks", e))?
                .is_file()
        {
            pins.push((name, file));
        }
    }
    Ok(pins)
}

/// An acquired directory, independently of its path designator or access policy.
#[derive(Debug)]
pub struct PinnedDirectory {
    file: File,
}
impl PinnedDirectory {
    pub fn open(path: &Path) -> std::io::Result<Self> {
        Ok(Self {
            file: open_at(
                libc::AT_FDCWD,
                path.as_os_str(),
                libc::O_PATH | libc::O_DIRECTORY,
            )?,
        })
    }
    pub fn descriptor(&self) -> &File {
        &self.file
    }
}

/// A destination pinned without creating or truncating it. Existing symlinks
/// resolve to their physical target; a new leaf is created exclusively relative
/// to its pinned parent, so a raced-in final symlink can never be followed.
#[derive(Debug)]
pub struct RecorderDestination {
    parent: PinnedDirectory,
    name: std::ffi::OsString,
    existing: Option<File>,
}
impl RecorderDestination {
    /// Two host sidecars must never overwrite the same leaf, including
    /// nonexistent leaves under aliased parents and existing hard links.
    pub(crate) fn check_distinct(&self, other: &Self) -> Result<(), RunRefusal> {
        let same_leaf = self.name == other.name
            && identity(&self.parent.file).map_err(|e| invalid("recorder", e))?
                == identity(&other.parent.file).map_err(|e| invalid("recorder", e))?;
        let same_file = match (&self.existing, &other.existing) {
            (Some(a), Some(b)) => {
                identity(a).map_err(|e| invalid("recorder", e))?
                    == identity(b).map_err(|e| invalid("recorder", e))?
            }
            _ => false,
        };
        if same_leaf || same_file {
            return Err(invalid(
                "trace_events",
                "destination overlaps another host recording or sidecar",
            ));
        }
        Ok(())
    }

    pub fn pin(path: &Path) -> Result<Self, RunRefusal> {
        let existing = match open_at(libc::AT_FDCWD, path.as_os_str(), libc::O_PATH) {
            Ok(file) => Some(file),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(invalid("recorder", e)),
        };
        let physical = match &existing {
            Some(file) => std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd()))
                .map_err(|e| invalid("recorder", e))?,
            None => path.to_path_buf(),
        };
        let name = physical
            .file_name()
            .ok_or_else(|| invalid("recorder", "destination has no file name"))?
            .to_owned();
        let parent = PinnedDirectory::open(
            physical
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
        )
        .map_err(|e| invalid("recorder", e))?;
        if let Some(file) = &existing {
            let child = open_at(parent.file.as_raw_fd(), &name, libc::O_PATH)
                .map_err(|e| invalid("recorder", e))?;
            if identity(&child).map_err(|e| invalid("recorder", e))?
                != identity(file).map_err(|e| invalid("recorder", e))?
            {
                return Err(invalid("recorder", "destination changed while pinning"));
            }
        }
        Ok(Self {
            parent,
            name,
            existing,
        })
    }
    /// Validate without creating a file. Control sockets use the same physical
    /// overlap checks as recorders and metrics, including hard-linked sinks.
    pub fn check_with_files(
        &self,
        grants: &[(&str, &PinnedDirectory)],
        files: &[(&str, &File)],
    ) -> Result<(), RunRefusal> {
        if let Some(destination) = &self.existing {
            let destination = identity(destination).map_err(|e| invalid("recorder", e))?;
            for (name, file) in files {
                if identity(file).map_err(|e| invalid("recorder.sinks", e))? == destination {
                    return Err(RunRefusal::RecorderOverlap {
                        mount: (*name).into(),
                    });
                }
            }
        }
        let recorder_ancestors = if grants.is_empty() {
            Ancestry::default()
        } else {
            ancestors(&self.parent.file).map_err(|e| invalid("recorder.ancestry", e))?
        };
        for (mount, grant) in grants {
            let grant_id = identity(&grant.file).map_err(|e| invalid("io.dirs", e))?;
            let overlap = recorder_ancestors.contains(&grant_id)
                || if let Some(file) = &self.existing {
                    contains_inode(
                        &grant.file,
                        identity(file).map_err(|e| invalid("recorder", e))?,
                    )
                    .map_err(|e| invalid("io.dirs", e))?
                } else {
                    false
                };
            if overlap {
                return Err(RunRefusal::RecorderOverlap {
                    mount: (*mount).into(),
                });
            }
        }
        Ok(())
    }

    pub fn check_control(
        &self,
        grants: &[(&str, &PinnedDirectory)],
        files: &[(&str, &File)],
    ) -> Result<(), RunRefusal> {
        self.check_with_files(grants, files)?;
        if self.existing.is_some() {
            return Err(invalid("placement.control", "destination already exists"));
        }
        Ok(())
    }

    /// Bind only after all overlap checks, never replacing an existing leaf.
    /// Rust std's UnixListener bind structure (os/unix/net/listener.rs:73-109),
    /// with chmod before listen and a pinned parent instead of a racy path lookup.
    pub fn bind_control(
        self,
        grants: &[(&str, &PinnedDirectory)],
        files: &[(&str, &File)],
    ) -> Result<ControlSocket, RunRefusal> {
        self.check_control(grants, files)?;
        let path = PathBuf::from(format!("/proc/self/fd/{}", self.parent.file.as_raw_fd()))
            .join(&self.name);
        let bytes = path.as_os_str().as_bytes();
        let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
        if bytes.len() >= address.sun_path.len() {
            return Err(invalid("placement.control", "socket path too long"));
        }
        address.sun_family = libc::AF_UNIX as _;
        for (out, byte) in address.sun_path.iter_mut().zip(bytes) {
            *out = *byte as _;
        }
        let fd = unsafe {
            libc::socket(
                libc::AF_UNIX,
                libc::SOCK_STREAM | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
                0,
            )
        };
        if fd < 0 {
            return Err(invalid(
                "placement.control",
                std::io::Error::last_os_error(),
            ));
        }
        let listener = unsafe { std::os::unix::net::UnixListener::from_raw_fd(fd) };
        let length = std::mem::offset_of!(libc::sockaddr_un, sun_path) + bytes.len() + 1;
        if unsafe { libc::bind(fd, (&raw const address).cast(), length as _) } < 0 {
            return Err(invalid(
                "placement.control",
                std::io::Error::last_os_error(),
            ));
        }
        let pin = open_at(
            self.parent.file.as_raw_fd(),
            &self.name,
            libc::O_PATH | libc::O_NOFOLLOW,
        )
        .map_err(|e| invalid("placement.control", e))?;
        let socket = ControlSocket {
            listener,
            destination: self,
            identity: identity(&pin).map_err(|e| invalid("placement.control", e))?,
        };
        let path = CString::new(format!("/proc/self/fd/{}", pin.as_raw_fd()))
            .map_err(|e| invalid("placement.control", e))?;
        // No connections are accepted before the endpoint is private.
        if unsafe { libc::chmod(path.as_ptr(), 0o600) } < 0 || unsafe { libc::listen(fd, 16) } < 0 {
            return Err(invalid(
                "placement.control",
                std::io::Error::last_os_error(),
            ));
        }
        Ok(socket)
    }

    /// Complete the recorder obligation before entry, checking every granted
    /// mount (including read-only mounts). No destination effects precede refusal.
    pub fn open(self, grants: &[(&str, &PinnedDirectory)]) -> Result<File, RunRefusal> {
        self.open_with_files(grants, &[])
    }
    /// Include explicitly acquired file-backed sinks, not merely directory grants.
    pub fn open_with_files(
        self,
        grants: &[(&str, &PinnedDirectory)],
        files: &[(&str, &File)],
    ) -> Result<File, RunRefusal> {
        self.check_with_files(grants, files)?;
        let file = match self.existing {
            Some(pin) => {
                let meta = pin.metadata().map_err(|e| invalid("recorder", e))?;
                if !meta.is_file() {
                    return Err(invalid("recorder", "destination must be a regular file"));
                }
                open_at(
                    libc::AT_FDCWD,
                    OsStr::new(&format!("/proc/self/fd/{}", pin.as_raw_fd())),
                    libc::O_WRONLY | libc::O_NONBLOCK,
                )
                .map_err(|e| invalid("recorder", e))?
            }
            None => open_at(
                self.parent.file.as_raw_fd(),
                &self.name,
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW,
            )
            .map_err(|e| invalid("recorder", e))?,
        };
        file.set_len(0).map_err(|e| invalid("recorder", e))?;
        Ok(file)
    }
}

/// A private, admitted Unix control socket. Removes only the inode it created.
pub struct ControlSocket {
    pub listener: std::os::unix::net::UnixListener,
    destination: RecorderDestination,
    identity: Identity,
}
impl Drop for ControlSocket {
    fn drop(&mut self) {
        let parent = self.destination.parent.file.as_raw_fd();
        if let Ok(pin) = open_at(
            parent,
            &self.destination.name,
            libc::O_PATH | libc::O_NOFOLLOW,
        ) && identity(&pin).ok() == Some(self.identity)
            && let Ok(name) = CString::new(self.destination.name.as_bytes())
        {
            unsafe { libc::unlinkat(parent, name.as_ptr(), 0) };
        }
    }
}

/// Storage owned by a directory recorder (the durable-journal adapter's seam).
/// Unlike a tape file, this owns descendants and must also refuse grants inside it.
#[derive(Debug)]
pub struct RecorderDirectory(PinnedDirectory);
impl RecorderDirectory {
    pub fn pin(path: &Path) -> Result<Self, RunRefusal> {
        PinnedDirectory::open(path)
            .map(Self)
            .map_err(|e| invalid("recorder", e))
    }
    pub fn check(self, grants: &[(&str, &PinnedDirectory)]) -> Result<PinnedDirectory, RunRefusal> {
        if grants.is_empty() {
            return Ok(self.0);
        }
        let recorder_id = identity(&self.0.file).map_err(|e| invalid("recorder", e))?;
        let recorder_ancestors =
            ancestors(&self.0.file).map_err(|e| invalid("recorder.ancestry", e))?;
        for (mount, grant) in grants {
            let grant_id = identity(&grant.file).map_err(|e| invalid("io.dirs", e))?;
            let grant_ancestors =
                ancestors(&grant.file).map_err(|e| invalid("io.dirs.ancestry", e))?;
            if recorder_ancestors.contains(&grant_id) || grant_ancestors.contains(&recorder_id) {
                return Err(RunRefusal::RecorderOverlap {
                    mount: (*mount).into(),
                });
            }
        }
        Ok(self.0)
    }
}

fn contains_inode(root: &File, wanted: Identity) -> std::io::Result<bool> {
    // Hard links and file bind mounts can expose an existing inode under a
    // grant independently of its recorder-side path and link count. Check all
    // existing destinations, without following symlinks or invoking device/FIFO
    // opens. Confined grant links cannot leave this tree.
    let mut work = vec![root.try_clone()?];
    let mut visited = BTreeSet::new();
    while let Some(directory) = work.pop() {
        // Separate bind views of the same directory can expose different child
        // mounts. Only skip a repeated inode within the same mount view.
        if !visited.insert((mount_id(&directory)?, identity(&directory)?)) {
            continue;
        }
        for entry in std::fs::read_dir(format!("/proc/self/fd/{}", directory.as_raw_fd()))? {
            let entry = entry?;
            let pin = open_at(
                directory.as_raw_fd(),
                &entry.file_name(),
                libc::O_PATH | libc::O_NOFOLLOW,
            )?;
            if identity(&pin)? == wanted {
                return Ok(true);
            }
            if pin.metadata()?.is_dir() {
                work.push(pin);
            }
        }
    }
    Ok(false)
}

#[derive(Default)]
struct Ancestry {
    identities: BTreeSet<Identity>,
    // Retain every compared inode, including ancestors, until admission finishes.
    pins: Vec<File>,
}
impl Ancestry {
    fn contains(&self, id: &Identity) -> bool {
        self.identities.contains(id)
    }
}
fn walk_parents(file: &File, result: &mut Ancestry) -> std::io::Result<()> {
    let mut current = file.try_clone()?;
    let mut visited = BTreeSet::new();
    loop {
        let id = identity(&current)?;
        result.identities.insert(id);
        result.pins.push(current.try_clone()?);
        if !visited.insert(id) {
            return Err(std::io::Error::other("directory ancestry cycle"));
        }
        let parent = open_at(
            current.as_raw_fd(),
            OsStr::new(".."),
            libc::O_PATH | libc::O_DIRECTORY,
        )?;
        if identity(&parent)? == id {
            return Ok(());
        }
        current = parent;
    }
}

/// One mountinfo row, borrowed: escaped paths are decoded only for the rows a check compares.
struct MountInfo<'a> {
    id: u64,
    device: &'a str,
    root: &'a str,
    point: &'a str,
}
fn mount_path(text: &str) -> std::io::Result<PathBuf> {
    let mut out = Vec::with_capacity(text.len());
    let mut bytes = text.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'\\' {
            let mut value = 0u16;
            for _ in 0..3 {
                let digit = bytes
                    .next()
                    .filter(|b| (b'0'..=b'7').contains(b))
                    .ok_or_else(|| std::io::Error::other("invalid mountinfo escape"))?;
                value = value * 8 + u16::from(digit - b'0');
            }
            out.push(u8::try_from(value).map_err(std::io::Error::other)?);
        } else {
            out.push(byte);
        }
    }
    Ok(std::ffi::OsString::from_vec(out).into())
}
/// The leading fields of every row. Mountinfo separates fields with ASCII spaces and escapes
/// them inside paths, so only a row's first six fields are scanned, never its mount options.
fn mounts(text: &str) -> std::io::Result<Vec<MountInfo<'_>>> {
    text.lines()
        .map(|line| {
            let mut fields = line.split_ascii_whitespace();
            let mut next = || {
                fields
                    .next()
                    .ok_or_else(|| std::io::Error::other("invalid mountinfo row"))
            };
            let id = next()?.parse().map_err(std::io::Error::other)?;
            next()?;
            let device = next()?;
            let root = next()?;
            let point = next()?;
            next()?;
            Ok(MountInfo {
                id,
                device,
                root,
                point,
            })
        })
        .collect()
}
fn mount_id(file: &File) -> std::io::Result<u64> {
    let info = std::fs::read_to_string(format!("/proc/self/fdinfo/{}", file.as_raw_fd()))?;
    info.lines()
        .find_map(|l| l.strip_prefix("mnt_id:\t"))
        .ok_or_else(|| std::io::Error::other("missing descriptor mount identity"))?
        .parse()
        .map_err(std::io::Error::other)
}
fn ancestors(file: &File) -> std::io::Result<Ancestry> {
    let mut result = Ancestry::default();
    walk_parents(file, &mut result)?;
    // `..` on a bind root crosses to its mountpoint parent, hiding the source
    // ancestry. Conversely a bind under a grant can expose otherwise outside
    // storage. Mountinfo locates all views of this directory in the filesystem;
    // names only locate candidates. Matching live descriptor dev/inode identity
    // is mandatory before any candidate contributes physical ancestors.
    let id = mount_id(file)?;
    let text = std::fs::read_to_string("/proc/self/mountinfo")?;
    let mounts = mounts(&text)?;
    let mount = mounts
        .iter()
        .find(|m| m.id == id)
        .ok_or_else(|| std::io::Error::other("descriptor mount is unavailable"))?;
    let (mount_root, mount_point) = (mount_path(mount.root)?, mount_path(mount.point)?);
    let path = std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd()))?;
    let suffix = path
        .strip_prefix(&mount_point)
        .map_err(std::io::Error::other)?;
    let physical = mount_root.join(suffix);
    let mut source_ancestry = mount_root == Path::new("/");
    for view in &mounts {
        if view.device != mount.device || view.id == mount.id {
            continue;
        }
        let view_root = mount_path(view.root)?;
        let Ok(suffix) = physical.strip_prefix(&view_root) else {
            continue;
        };
        let Ok(alias) = PinnedDirectory::open(&mount_path(view.point)?.join(suffix)) else {
            continue;
        };
        if mount_id(&alias.file)? == view.id && identity(&alias.file)? == identity(file)? {
            walk_parents(&alias.file, &mut result)?;
            source_ancestry |= mount_root != view_root && mount_root.starts_with(&view_root);
        }
    }
    if !source_ancestry {
        return Err(std::io::Error::other(
            "cannot establish bind-mount source ancestry",
        ));
    }
    Ok(result)
}

#[cfg(test)]
mod mountinfo_tests {
    use super::{mount_path, mounts};
    use std::path::Path;

    /// proc(5)'s mountinfo example, a row whose paths escape a space and a backslash, and a
    /// row with kilobytes of options (overlay lowerdirs) that the parser never scans.
    #[test]
    fn rows_parse_their_leading_fields_and_decode_paths_on_demand() {
        let options = "lowerdir=".to_owned() + &"/l".repeat(2000);
        let text = format!(
            "36 35 98:0 /mnt1 /mnt2 rw,noatime master:1 - ext3 /dev/root rw,errors=continue\n\
             37 36 0:5 /a\\040b /m\\134x rw - tmpfs tmpfs rw\n\
             38 36 0:61 / /merged rw - overlay overlay rw,{options}\n"
        );
        let rows = mounts(&text).unwrap();
        let fields: Vec<_> = rows
            .iter()
            .map(|r| (r.id, r.device, r.root, r.point))
            .collect();
        assert_eq!(
            fields,
            [
                (36, "98:0", "/mnt1", "/mnt2"),
                (37, "0:5", r"/a\040b", r"/m\134x"),
                (38, "0:61", "/", "/merged"),
            ]
        );
        assert_eq!(mount_path(rows[1].root).unwrap(), Path::new("/a b"));
        assert_eq!(mount_path(rows[1].point).unwrap(), Path::new(r"/m\x"));
        assert!(mount_path(r"/bad\08").is_err());
        assert!(
            mounts("36 35 98:0 /mnt1 /mnt2\n").is_err(),
            "a row has six fields"
        );
        assert!(
            mounts("x 35 98:0 / / rw\n").is_err(),
            "a row's id is a number"
        );
        assert!(mounts("").unwrap().is_empty());
    }
}
