//! Change events behind `file.watch` and `watcher.next`. Every watcher has a [`WatchQueue`]:
//! an event equal to the last one queued is coalesced into it, and past [`QUEUE`] pending
//! events the queue ends with one `Overflow()`, dropping later events until it is read.
//! A test tree's watchers receive only the events `test.dir_event` injects (`io.rs`); a
//! host Dir's watchers read inotify ([`HostWatch`], `io_real.rs`). Paths are tree paths
//! here and become relative to the watcher's Dir when `watcher.next` returns them.
use crate::io::IoError;
use std::collections::{BTreeMap, VecDeque};
use std::fs::File;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

/// A change under a watched path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WatchEvent {
    Created(String),
    Modified(String),
    Removed(String),
    Renamed(String, String),
    Overflow,
}

/// Pending events a watcher keeps before its queue overflows.
pub const QUEUE: usize = 1024;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WatchQueue {
    events: VecDeque<WatchEvent>,
    /// An `Overflow()` is queued and unread: every later event is dropped.
    overflowed: bool,
}
impl WatchQueue {
    pub fn push(&mut self, event: WatchEvent) {
        if self.overflowed || self.events.back() == Some(&event) {
            return;
        }
        if event == WatchEvent::Overflow || self.events.len() >= QUEUE {
            self.overflowed = true;
            self.events.push_back(WatchEvent::Overflow);
        } else {
            self.events.push_back(event);
        }
    }
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
    pub fn pop(&mut self) -> Option<WatchEvent> {
        let event = self.events.pop_front()?;
        if event == WatchEvent::Overflow {
            self.overflowed = false;
        }
        Some(event)
    }
}

/// Whether a watch of `root` sees a change at `path`: the root itself, an entry directly in
/// it, or with `recursive` any path below it.
fn covers(root: &str, recursive: bool, path: &str) -> bool {
    if path == root {
        return true;
    }
    let rest = if root.is_empty() {
        Some(path)
    } else {
        path.strip_prefix(root)
            .and_then(|rest| rest.strip_prefix('/'))
    };
    rest.is_some_and(|rest| recursive || !rest.contains('/'))
}

/// `event` as a watch of `root` sees it, or `None` when it is outside the watch. A rename
/// into the watch is a creation there, and one out of it a removal.
pub fn seen(root: &str, recursive: bool, event: &WatchEvent) -> Option<WatchEvent> {
    use WatchEvent::*;
    let covered = |path: &str| covers(root, recursive, path);
    match event {
        Created(p) | Modified(p) | Removed(p) => covered(p).then(|| event.clone()),
        Renamed(from, to) => match (covered(from), covered(to)) {
            (true, true) => Some(event.clone()),
            (true, false) => Some(Removed(from.clone())),
            (false, true) => Some(Created(to.clone())),
            (false, false) => None,
        },
        Overflow => Some(Overflow),
    }
}

/// `event`'s tree paths relative to Dir root `dir`, which contains every watched path.
pub fn relative_to(dir: &str, event: WatchEvent) -> WatchEvent {
    use WatchEvent::*;
    let local = |path: String| -> String {
        if dir.is_empty() {
            path
        } else if path == dir {
            String::new()
        } else {
            path.strip_prefix(&format!("{dir}/"))
                .map_or(path.clone(), str::to_owned)
        }
    };
    match event {
        Created(p) => Created(local(p)),
        Modified(p) => Modified(local(p)),
        Removed(p) => Removed(local(p)),
        Renamed(a, b) => Renamed(local(a), local(b)),
        Overflow => Overflow,
    }
}

/// `event` with each path mapped by `f`, or `None` when `f` rejects one.
pub fn map_paths(event: &WatchEvent, f: impl Fn(&str) -> Option<String>) -> Option<WatchEvent> {
    use WatchEvent::*;
    Some(match event {
        Created(p) => Created(f(p)?),
        Modified(p) => Modified(f(p)?),
        Removed(p) => Removed(f(p)?),
        Renamed(a, b) => Renamed(f(a)?, f(b)?),
        Overflow => Overflow,
    })
}

const MASK: u32 = libc::IN_CREATE
    | libc::IN_DELETE
    | libc::IN_MODIFY
    | libc::IN_MOVED_FROM
    | libc::IN_MOVED_TO
    | libc::IN_DELETE_SELF
    | libc::IN_MOVE_SELF
    | libc::IN_EXCL_UNLINK;

/// One host watcher: an inotify instance with a watch on the watched path and, when
/// `recursive`, on every directory below it, including directories created later.
///
/// inotify reports content writes (`IN_MODIFY`) as Modified, creations and removals of
/// entries, and the root's own removal or move as Removed. A move inside the watch is
/// Renamed when its `IN_MOVED_FROM` and `IN_MOVED_TO` arrive in one read, as the kernel
/// queues them; otherwise its halves are a removal and a creation. A queue overflow in
/// the kernel is `Overflow()`. A directory created or moved into a recursive watch is
/// watched from when its event is read, so entries made in it before then are not seen.
#[derive(Debug)]
pub(crate) struct HostWatch {
    fd: OwnedFd,
    recursive: bool,
    root: String,
    root_descriptor: i32,
    /// The tree path of each watched directory or file, by watch descriptor.
    watched: BTreeMap<i32, String>,
    queue: WatchQueue,
}
impl HostWatch {
    /// Watch `target`, pinned at tree path `root`, and `directories` below it.
    pub(crate) fn new(
        target: &File,
        root: &str,
        recursive: bool,
        directories: Vec<(File, String)>,
    ) -> Result<Self, IoError> {
        // SAFETY: inotify_init1 has no preconditions.
        let raw = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        if raw < 0 {
            return Err(IoError::Fault(1));
        }
        // SAFETY: raw is a fresh descriptor nothing else owns.
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        let mut watch = Self {
            fd,
            recursive,
            root: root.to_owned(),
            root_descriptor: -1,
            watched: BTreeMap::new(),
            queue: WatchQueue::default(),
        };
        watch.root_descriptor = watch.add(target, root.to_owned())?;
        for (directory, path) in directories {
            watch.add(&directory, path)?;
        }
        Ok(watch)
    }
    fn add(&mut self, pinned: &File, path: String) -> Result<i32, IoError> {
        let proc = std::ffi::CString::new(format!("/proc/self/fd/{}", pinned.as_raw_fd()))
            .map_err(|_| IoError::Fault(1))?;
        // SAFETY: a live inotify descriptor and a terminated path naming a pinned inode.
        let wd = unsafe { libc::inotify_add_watch(self.fd.as_raw_fd(), proc.as_ptr(), MASK) };
        if wd < 0 {
            return Err(match std::io::Error::last_os_error().raw_os_error() {
                Some(libc::EACCES) => IoError::PermissionDenied,
                Some(libc::ENOENT) => IoError::NotFound,
                Some(code) => IoError::Fault(u32::try_from(code).unwrap_or(1)),
                None => IoError::Fault(1),
            });
        }
        self.watched.insert(wd, path);
        Ok(wd)
    }
    pub(crate) fn descriptor(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
    /// The next queued event, after reading every event inotify has: `None` when none has
    /// arrived. `open` pins a new directory of a recursive watch by tree path.
    pub(crate) fn next(
        &mut self,
        open: &mut dyn FnMut(&str) -> Option<File>,
    ) -> Result<Option<WatchEvent>, IoError> {
        if self.queue.events.is_empty() {
            self.drain(open)?;
        }
        Ok(self.queue.pop())
    }
    /// Wait until the inotify descriptor is readable.
    pub(crate) fn block(&self) -> Result<(), IoError> {
        let mut poll = libc::pollfd {
            fd: self.fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        loop {
            // SAFETY: one writable pollfd.
            if unsafe { libc::poll(&mut poll, 1, -1) } >= 0 {
                return Ok(());
            }
            if std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
                return Err(IoError::Fault(1));
            }
        }
    }
    fn drain(&mut self, open: &mut dyn FnMut(&str) -> Option<File>) -> Result<(), IoError> {
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            // SAFETY: the buffer is writable for its length.
            let n = unsafe {
                libc::read(
                    self.fd.as_raw_fd(),
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                )
            };
            if n < 0 {
                return match std::io::Error::last_os_error().raw_os_error() {
                    Some(libc::EAGAIN) => Ok(()),
                    Some(libc::EINTR) => continue,
                    _ => Err(IoError::Fault(1)),
                };
            }
            self.translate(&buffer[..n as usize], open);
        }
    }
    fn translate(&mut self, mut bytes: &[u8], open: &mut dyn FnMut(&str) -> Option<File>) {
        use WatchEvent::*;
        let header = size_of::<libc::inotify_event>();
        // A move's first half, waiting for its second in this read.
        let mut moved: Option<(u32, String, bool)> = None;
        while bytes.len() >= header {
            // SAFETY: the kernel writes whole events; the header may be unaligned here.
            let event: libc::inotify_event =
                unsafe { std::ptr::read_unaligned(bytes.as_ptr().cast()) };
            let length = header + event.len as usize;
            let name = bytes[header..length.min(bytes.len())]
                .split(|b| *b == 0)
                .next()
                .and_then(|name| std::str::from_utf8(name).ok())
                .unwrap_or_default()
                .to_owned();
            bytes = &bytes[length.min(bytes.len())..];
            let directory = event.mask & libc::IN_ISDIR != 0;
            if event.mask & libc::IN_MOVED_TO == 0
                && let Some((_, from, _)) = moved.take()
            {
                self.queue.push(Removed(from));
            }
            if event.mask & libc::IN_Q_OVERFLOW != 0 {
                self.queue.push(Overflow);
                continue;
            }
            if event.mask & libc::IN_IGNORED != 0 {
                self.watched.remove(&event.wd);
                continue;
            }
            let Some(base) = self.watched.get(&event.wd).cloned() else {
                continue;
            };
            let path = match (base.is_empty(), name.is_empty()) {
                (_, true) => base.clone(),
                (true, false) => name,
                (false, false) => format!("{base}/{name}"),
            };
            let mask = event.mask;
            if mask & libc::IN_CREATE != 0 {
                self.queue.push(Created(path.clone()));
                self.descend(directory, &path, open);
            } else if mask & libc::IN_MODIFY != 0 {
                self.queue.push(Modified(path));
            } else if mask & libc::IN_DELETE != 0 {
                self.queue.push(Removed(path));
            } else if mask & libc::IN_MOVED_FROM != 0 {
                moved = Some((event.cookie, path, directory));
            } else if mask & libc::IN_MOVED_TO != 0 {
                match moved.take() {
                    Some((cookie, from, true)) if cookie == event.cookie => {
                        self.moved_directory(&from, &path);
                        self.queue.push(Renamed(from, path));
                    }
                    Some((cookie, from, false)) if cookie == event.cookie => {
                        self.queue.push(Renamed(from, path));
                    }
                    other => {
                        if let Some((_, from, _)) = other {
                            self.queue.push(Removed(from));
                        }
                        self.queue.push(Created(path.clone()));
                        self.descend(directory, &path, open);
                    }
                }
            } else if mask & (libc::IN_DELETE_SELF | libc::IN_MOVE_SELF) != 0
                && event.wd == self.root_descriptor
            {
                self.queue.push(Removed(self.root.clone()));
            }
        }
        if let Some((_, from, _)) = moved {
            self.queue.push(Removed(from));
        }
    }
    fn descend(&mut self, directory: bool, path: &str, open: &mut dyn FnMut(&str) -> Option<File>) {
        if directory
            && self.recursive
            && let Some(pinned) = open(path)
        {
            let _ = self.add(&pinned, path.to_owned());
        }
    }
    /// A directory moved inside the watch keeps its descriptors under its new path.
    fn moved_directory(&mut self, from: &str, to: &str) {
        for path in self.watched.values_mut() {
            if path == from {
                *path = to.to_owned();
            } else if let Some(rest) = path.strip_prefix(&format!("{from}/")) {
                *path = format!("{to}/{rest}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::WatchEvent::*;
    use super::*;

    #[test]
    fn queues_coalesce_repeats_and_overflow_once_until_read() {
        let mut queue = WatchQueue::default();
        queue.push(Modified("a".into()));
        queue.push(Modified("a".into()));
        queue.push(Modified("b".into()));
        queue.push(Modified("a".into()));
        assert_eq!(queue.pop(), Some(Modified("a".into())));
        assert_eq!(queue.pop(), Some(Modified("b".into())));
        assert_eq!(queue.pop(), Some(Modified("a".into())));
        assert_eq!(queue.pop(), None);
        for i in 0..QUEUE + 10 {
            queue.push(Created(i.to_string()));
        }
        for i in 0..QUEUE {
            assert_eq!(queue.pop(), Some(Created(i.to_string())));
        }
        assert_eq!(queue.pop(), Some(Overflow));
        assert_eq!(queue.pop(), None);
        queue.push(Removed("x".into()));
        assert_eq!(queue.pop(), Some(Removed("x".into())));
    }

    #[test]
    fn a_watch_sees_its_root_its_entries_and_with_recursion_everything_below() {
        assert!(covers("d", false, "d"));
        assert!(covers("d", false, "d/a"));
        assert!(!covers("d", false, "d/a/b"));
        assert!(covers("d", true, "d/a/b"));
        assert!(!covers("d", true, "dd/a"));
        assert!(covers("", false, "a"));
        assert!(!covers("", false, "a/b"));
        let rename = Renamed("d/a".into(), "e/a".into());
        assert_eq!(seen("d", false, &rename), Some(Removed("d/a".into())));
        assert_eq!(seen("e", false, &rename), Some(Created("e/a".into())));
        assert_eq!(seen("f", false, &rename), None);
        assert_eq!(relative_to("d", Created("d/a".into())), Created("a".into()));
        assert_eq!(
            map_paths(&Created("a".into()), |p| {
                hirsel_run_config::confined_relative("d", p)
            }),
            Some(Created("d/a".into()))
        );
        assert_eq!(
            map_paths(&Created("../a".into()), |p| {
                hirsel_run_config::confined_relative("d", p)
            }),
            None
        );
    }
}
