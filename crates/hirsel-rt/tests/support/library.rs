//! Typed request builders used only by runtime integration tests.
use hirsel_rt::io::{Adapter, AdapterError, Completion, IoError, Request};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Io(pub u64);
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Stdout(pub u64);
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Dir(pub u64);
#[derive(Debug, Eq, PartialEq)]
pub struct File(pub u64);
#[derive(Debug, Eq, PartialEq)]
pub struct AtomicFile(pub u64);

/// Typed requests for integration tests; production uses the compiled ABI entries.
pub struct Library<A: Adapter>(pub A);
macro_rules! typed {
    ($name:ident($($arg:ident : $ty:ty),* $(,)?) -> $result:ty, $request:expr, $variant:ident, $convert:expr) => {
        pub fn $name(&mut self, $($arg: $ty),*) -> Result<$result, AdapterError> {
            if let Completion::$variant(value) = self.0.call($request)? { Ok(($convert)(value)) } else { Err(AdapterError::InvalidCompletion) }
        }
    };
}
impl<A: Adapter> Library<A> {
    typed!(
        stdout(io: Io) -> Result<Stdout, IoError>,
        Request::Stdout { io: io.0 },
        Handle,
        |v: Result<u64, IoError>| v.map(Stdout)
    );
    typed!(
        directory(io: Io, path: &str) -> Result<Dir, IoError>,
        Request::Directory {
            io: io.0,
            path: path.into()
        },
        Handle,
        |v: Result<u64, IoError>| v.map(Dir)
    );
    typed!(
        narrow_dir(directory: Dir, path: &str) -> Result<Dir, IoError>,
        Request::NarrowDir {
            directory: directory.0,
            path: path.into()
        },
        Handle,
        |v: Result<u64, IoError>| v.map(Dir)
    );
    typed!(
        print(stdout: Stdout, text: &str) -> Result<(), IoError>,
        Request::Print {
            stdout: stdout.0,
            bytes: text.as_bytes().to_vec()
        },
        Unit,
        |v| v
    );
    typed!(
        open(directory: Dir, path: &str) -> Result<File, IoError>,
        Request::Open {
            directory: directory.0,
            path: path.into()
        },
        Handle,
        |v: Result<u64, IoError>| v.map(File)
    );
    typed!(
        narrow_file(directory: Dir, file: File) -> Option<File>,
        Request::NarrowFile {
            directory: directory.0,
            file: file.0
        },
        OptionalFile,
        |v: Option<u64>| v.map(File)
    );
    pub fn walk(&mut self, directory: Dir) -> Result<Result<Vec<String>, IoError>, AdapterError> {
        let mut walk = hirsel_rt::kernels::walk::Walk::default();
        let mut next = Some(String::new());
        let mut paths = vec![];
        loop {
            if let Some(path) = next.take() {
                let Completion::Entries(entries) = self.0.call(Request::ReadDir {
                    directory: directory.0,
                    path,
                })?
                else {
                    panic!("read_dir completion")
                };
                let entries = match entries {
                    Ok(entries) => entries,
                    Err(error) => return Ok(Err(error)),
                };
                walk.push(
                    entries
                        .into_iter()
                        .map(|e| hirsel_rt::kernels::walk::Entry {
                            path: e.path,
                            kind: e.kind as u32,
                        })
                        .collect(),
                );
            }
            match walk.next_entry() {
                Some(entry) if entry.kind == 1 => next = Some(entry.path),
                Some(entry) if matches!(entry.kind, 0 | 2) => paths.push(entry.path),
                Some(_) => return Ok(Err(IoError::Fault(1))),
                None => return Ok(Ok(paths)),
            }
        }
    }
    typed!(
        size(directory: Dir, path: &str) -> Result<u64, IoError>,
        Request::Size {
            directory: directory.0,
            path: path.into()
        },
        Size,
        |v| v
    );
    typed!(
        read(directory: Dir, path: &str) -> Result<Vec<u8>, IoError>,
        Request::Read {
            directory: directory.0,
            path: path.into()
        },
        Bytes,
        |v| v
    );
    typed!(
        write(directory: Dir, path: &str, bytes: Vec<u8>) -> Result<(), IoError>,
        Request::Write {
            directory: directory.0,
            path: path.into(),
            bytes
        },
        Unit,
        |v| v
    );
    typed!(
        remove(directory: Dir, path: &str) -> Result<(), IoError>,
        Request::Remove {
            directory: directory.0,
            path: path.into()
        },
        Unit,
        |v| v
    );
    typed!(
        create_atomic(directory: Dir, path: &str) -> Result<AtomicFile, IoError>,
        Request::CreateAtomic {
            directory: directory.0,
            path: path.into()
        },
        Handle,
        |v: Result<u64, IoError>| v.map(AtomicFile)
    );
    typed!(
        write_all(directory: Dir, file: &mut AtomicFile, bytes: &[u8]) -> Result<(), IoError>,
        Request::WriteAll {
            directory: directory.0,
            file: file.0,
            bytes: bytes.into()
        },
        Unit,
        |v| v
    );
    typed!(
        commit(directory: Dir, file: AtomicFile) -> Result<(), IoError>,
        Request::Commit {
            directory: directory.0,
            file: file.0
        },
        Unit,
        |v| v
    );
    typed!(
        close_file(file: File) -> Result<(), IoError>,
        Request::Close { handle: file.0 },
        Unit,
        |v| v
    );
    typed!(
        abort(file: AtomicFile) -> Result<(), IoError>,
        Request::Close { handle: file.0 },
        Unit,
        |v| v
    );
}
