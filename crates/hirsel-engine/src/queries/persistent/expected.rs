//! Immutable VerifyStore expectations outlive preflight, not their heap buffers.
//! Go's io.copyBuffer (src/io/io.go:407-450) bounds scratch independently of
//! input size. Here the sink compares exact bytes rather than copying them.
use crate::memo::Reads;
use crate::store::Variant;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

const CHUNK: usize = 32 * 1024;

/// One backing file per live set of expectations. Last-handle release closes
/// and removes it, including on query failure or when an engine is dropped.
pub(crate) struct Spool {
    file: Mutex<Option<File>>,
    path: PathBuf,
}
impl Drop for Spool {
    fn drop(&mut self) {
        self.file
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        let _ = std::fs::remove_file(&self.path);
    }
}
impl Spool {
    fn new(directory: &Path) -> io::Result<Arc<Self>> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        loop {
            let path = directory.join(format!(
                "hirsel-verify-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let mut options = OpenOptions::new();
            options.read(true).write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&path) {
                Ok(file) => {
                    return Ok(Arc::new(Self {
                        file: Mutex::new(Some(file)),
                        path,
                    }));
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
    }
}

#[derive(Clone)]
pub(crate) struct Expected(Result<Record, Arc<str>>);
#[derive(Clone)]
struct Record {
    spool: Arc<Spool>,
    offset: u64,
    reads: usize,
    payload: usize,
}
impl Expected {
    /// The host chooses scratch placement through std::env::temp_dir (TMPDIR
    /// on Unix). No store hit or enabled verification means no scratch I/O.
    /// Failure remains a verification obligation, never a silent cache miss.
    pub(crate) fn capture(variant: Variant, backing: &mut Weak<Spool>) -> Self {
        Self(
            Self::capture_in(variant, backing, &std::env::temp_dir())
                .map_err(|error| Arc::from(error.to_string())),
        )
    }

    fn capture_in(
        variant: Variant,
        backing: &mut Weak<Spool>,
        directory: &Path,
    ) -> io::Result<Record> {
        let spool = match backing.upgrade() {
            Some(spool) => spool,
            None => {
                let spool = Spool::new(directory)?;
                *backing = Arc::downgrade(&spool);
                spool
            }
        };
        let mut reads = Vec::new();
        variant.reads.encode(&mut reads);
        let offset = {
            let mut guard = spool.file.lock().unwrap_or_else(|e| e.into_inner());
            let file = guard.as_mut().expect("live spool");
            let offset = file.seek(SeekFrom::End(0))?;
            file.write_all(&reads)?;
            file.write_all(&variant.payload)?;
            offset
        };
        Ok(Record {
            spool,
            offset,
            reads: reads.len(),
            payload: variant.payload.len(),
        })
    }

    pub(crate) fn matches(&self, reads: &Reads, payload: &[u8]) -> io::Result<bool> {
        let record = self
            .0
            .as_ref()
            .map_err(|error| io::Error::other(error.to_string()))?;
        let mut encoded = Vec::new();
        reads.encode(&mut encoded);
        if encoded.len() != record.reads || payload.len() != record.payload {
            return Ok(false);
        }
        let mut guard = record.spool.file.lock().unwrap_or_else(|e| e.into_inner());
        let file = guard.as_mut().expect("live spool");
        file.seek(SeekFrom::Start(record.offset))?;
        let mut scratch = [0; CHUNK];
        for bytes in [encoded.as_slice(), payload] {
            for chunk in bytes.chunks(CHUNK) {
                file.read_exact(&mut scratch[..chunk.len()])?;
                if scratch[..chunk.len()] != *chunk {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_comparison_covers_empty_short_and_multiple_chunks_and_releases_backing() {
        let mut backing = Weak::new();
        for size in [0, 1, CHUNK - 1, CHUNK, CHUNK + 1, 3 * CHUNK] {
            let payload: Arc<[u8]> = vec![17; size].into();
            let weak = Arc::downgrade(&payload);
            let expected = Expected::capture(
                Variant {
                    reads: Reads::empty(),
                    payload,
                },
                &mut backing,
            );
            assert!(weak.upgrade().is_none());
            let path = backing.upgrade().unwrap().path.clone();
            let mut actual = vec![17; size];
            assert!(expected.matches(&Reads::empty(), &actual).unwrap());
            actual.push(17);
            assert!(!expected.matches(&Reads::empty(), &actual).unwrap());
            actual.pop();
            if size > 0 {
                actual[size - 1] ^= 1;
                assert!(!expected.matches(&Reads::empty(), &actual).unwrap());
            }
            let clone = expected.clone();
            drop(expected);
            assert!(path.exists());
            drop(clone);
            assert!(!path.exists());
            assert!(backing.upgrade().is_none());
        }
    }

    #[test]
    fn missing_scratch_and_truncated_backing_are_errors() {
        let mut backing = Weak::new();
        let payload = vec![1; CHUNK + 1];
        let record = || Variant {
            reads: Reads::empty(),
            payload: payload.clone().into(),
        };
        let missing =
            std::env::temp_dir().join(format!("hirsel-missing-scratch-{}", std::process::id()));
        assert!(Expected::capture_in(record(), &mut backing, &missing).is_err());
        assert!(backing.upgrade().is_none());
        let expected = Expected::capture(record(), &mut backing);
        let spool = backing.upgrade().unwrap();
        spool
            .file
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .set_len(0)
            .unwrap();
        assert!(expected.matches(&Reads::empty(), &payload).is_err());
    }

    #[test]
    fn same_length_reads_are_compared_exactly_and_capture_errors_remain_errors() {
        let reads = |byte| {
            let mut reads = crate::memo::ReadsBuilder::new();
            reads.record(
                crate::ReadKey::Environment,
                hirsel_syntax::ContentHash([byte; 32]),
            );
            reads.finish().unwrap()
        };
        let mut backing = Weak::new();
        let expected = Expected::capture(
            Variant {
                reads: reads(1),
                payload: Arc::from(&b"body"[..]),
            },
            &mut backing,
        );
        assert!(expected.matches(&reads(1), b"body").unwrap());
        assert!(!expected.matches(&reads(2), b"body").unwrap());
        let failed = Expected(Err(Arc::from("injected scratch write failure")));
        assert!(failed.matches(&reads(1), b"body").is_err());
    }
}
