//! Bounded native linker inputs in the ordinary selected artifact store.
//! Byte transport and atomic file materialization confer no native authority.
use hirsel_artifact_store::{Domain, MAX_RECORD, Output, Reference};
use hirsel_codec::{Snapshot, snapshot_struct};
use hirsel_engine::store::ResultStore;
use hirsel_syntax::ContentHash;
use std::path::Path;

const BLOB: Domain = Domain {
    kind: 0x4f000004,
    schema: 1,
};
const CHUNK: Domain = Domain {
    kind: 0x4f000005,
    schema: 1,
};

fn require(
    store: &dyn ResultStore,
    reference: Reference,
) -> Result<hirsel_artifact_store::Bytes, String> {
    store
        .get_content(reference)
        .ok_or_else(|| "missing or corrupt native product content".into())
}
fn error(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn put_record<T: Snapshot>(
    store: &dyn ResultStore,
    domain: Domain,
    value: &T,
) -> Result<Reference, String> {
    store
        .put_content(domain, hirsel_codec::to_text(value).as_bytes())
        .ok_or_else(|| "native product publication failed".into())
}
fn get_record<T: Snapshot>(
    store: &dyn ResultStore,
    domain: Domain,
    reference: Reference,
) -> Result<T, String> {
    if reference.domain != domain {
        return Err("wrong native product domain/schema".into());
    }
    let bytes = require(store, reference)?;
    let text = std::str::from_utf8(&bytes).map_err(error)?;
    let value = hirsel_codec::from_text(text).map_err(|e| e.message)?;
    if hirsel_codec::to_text(&value) != text {
        return Err("noncanonical native product".into());
    }
    Ok(value)
}

// A blob is an ordinary bounded record of checked chunk references. Large object
// and executable bytes never require raising the store's per-record limit.
fn put_blob(store: &dyn ResultStore, bytes: &[u8]) -> Result<Reference, String> {
    let chunks = bytes
        .chunks(MAX_RECORD)
        .map(|chunk| {
            store
                .put_content(CHUNK, chunk)
                .map(WireRef::from)
                .ok_or_else(|| "native chunk publication failed".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    put_record(
        store,
        BLOB,
        &BlobRecord {
            len: bytes.len() as u64,
            chunks,
        },
    )
}

/// Snapshot owned runtime/foreign input bytes in the same ordinary provider.
pub fn store_link_input(store: &dyn ResultStore, bytes: &[u8]) -> Result<Reference, String> {
    put_blob(store, bytes)
}

/// Snapshot a file/stream with one bounded chunk buffer, using the exact same
/// blob/chunk encoding as byte-slice inputs. Short reads do not split chunks.
pub fn store_link_input_reader(
    store: &dyn ResultStore,
    mut reader: impl std::io::Read,
) -> Result<Reference, String> {
    use std::io::Read;
    let mut chunks = Vec::new();
    let mut len = 0u64;
    let mut buffer = Vec::with_capacity(MAX_RECORD);
    loop {
        buffer.clear();
        reader
            .by_ref()
            .take(MAX_RECORD as u64)
            .read_to_end(&mut buffer)
            .map_err(error)?;
        if buffer.is_empty() {
            break;
        }
        len = len
            .checked_add(buffer.len() as u64)
            .ok_or("native blob length overflow")?;
        let reference = store
            .put_content(CHUNK, &buffer)
            .ok_or("native chunk publication failed")?;
        if CHUNK.reference(&buffer) != Some(reference) {
            return Err("native chunk content identity differs".into());
        }
        chunks.push(WireRef::from(reference));
    }
    put_record(store, BLOB, &BlobRecord { len, chunks })
}

/// Restore a frozen linker/tool input through the same atomic writer as
/// executable publication. The original mutable path is never reopened.
pub fn materialize_link_input(
    store: &dyn ResultStore,
    reference: Reference,
    destination: &Path,
    executable: bool,
) -> Result<(), String> {
    materialize_blob(store, reference, destination, executable)
}

/// Materialize a previously snapshotted input, never reopen its mutable path.
pub fn link_input_bytes(store: &dyn ResultStore, reference: Reference) -> Result<Vec<u8>, String> {
    read_blob(store, reference)
}
fn blob_chunks(
    store: &dyn ResultStore,
    reference: Reference,
) -> Result<Vec<hirsel_artifact_store::Bytes>, String> {
    let blob: BlobRecord = get_record(store, BLOB, reference)?;
    let mut total = 0u64;
    let mut chunks = Vec::with_capacity(blob.chunks.len());
    for (i, chunk) in blob.chunks.iter().enumerate() {
        let chunk: Reference = chunk.clone().into();
        if chunk.domain != CHUNK
            || chunk.len == 0
            || (i + 1 != blob.chunks.len() && chunk.len != MAX_RECORD as u64)
        {
            return Err("invalid native blob chunk".into());
        }
        total = total
            .checked_add(chunk.len)
            .ok_or("native blob length overflow")?;
        chunks.push(require(store, chunk)?);
    }
    if total != blob.len {
        return Err("native blob length mismatch".into());
    }
    Ok(chunks)
}
fn read_blob(store: &dyn ResultStore, reference: Reference) -> Result<Vec<u8>, String> {
    let chunks = blob_chunks(store, reference)?;
    let len = chunks.iter().map(|c| c.len()).sum();
    let mut bytes = Vec::with_capacity(len);
    for chunk in chunks {
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn materialize_blob(
    store: &dyn ResultStore,
    reference: Reference,
    destination: &Path,
    executable: bool,
) -> Result<(), String> {
    use std::io::{Read, Write};
    let chunks = blob_chunks(store, reference)?;
    if executable && chunks.is_empty() {
        return Err("empty executable output".into());
    }
    let matches = |file: &mut std::fs::File| -> std::io::Result<bool> {
        let mut buffer = vec![0; 64 << 10];
        for chunk in &chunks {
            for part in chunk.chunks(buffer.len()) {
                file.read_exact(&mut buffer[..part.len()])?;
                if buffer[..part.len()] != *part {
                    return Ok(false);
                }
            }
        }
        Ok(file.read(&mut buffer[..1])? == 0)
    };
    let permissions = |file: &std::fs::File| -> std::io::Result<bool> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            Ok((file.metadata()?.permissions().mode() & 0o111 != 0) == executable)
        }
        #[cfg(not(unix))]
        {
            let _ = file;
            Ok(true)
        }
    };
    if let Ok(mut file) = std::fs::File::open(destination)
        && file.metadata().map_err(error)?.is_file()
        && permissions(&file).map_err(error)?
        && matches(&mut file).unwrap_or(false)
    {
        return Ok(());
    }
    let parent = destination.parent().ok_or("executable has no parent")?;
    std::fs::create_dir_all(parent).map_err(error)?;
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let temporary = parent.join(format!(
        ".hirsel-executable-{}-{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let result = (|| -> Result<(), String> {
        let mut file = std::fs::File::options()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(error)?;
        for chunk in &chunks {
            file.write_all(chunk).map_err(error)?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(if executable {
                0o755
            } else {
                0o644
            }))
            .map_err(error)?;
        }
        file.sync_all().map_err(error)?;
        std::fs::rename(&temporary, destination).map_err(error)?;
        std::fs::File::open(parent)
            .and_then(|f| f.sync_all())
            .map_err(error)?;
        let mut file = std::fs::File::open(destination).map_err(error)?;
        if !permissions(&file).map_err(error)? || !matches(&mut file).map_err(error)? {
            return Err("materialized native file failed readiness".into());
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

#[derive(Clone)]
struct WireRef {
    output: ContentHash,
    kind: u32,
    schema: u32,
    len: u64,
}
impl From<Reference> for WireRef {
    fn from(r: Reference) -> Self {
        Self {
            output: r.output.0,
            kind: r.domain.kind,
            schema: r.domain.schema,
            len: r.len,
        }
    }
}
impl From<WireRef> for Reference {
    fn from(r: WireRef) -> Self {
        Self {
            output: Output(r.output),
            domain: Domain {
                kind: r.kind,
                schema: r.schema,
            },
            len: r.len,
        }
    }
}
struct BlobRecord {
    len: u64,
    chunks: Vec<WireRef>,
}
snapshot_struct!(WireRef {
    output,
    kind,
    schema,
    len
});
snapshot_struct!(BlobRecord { len, chunks });

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_engine::store::DirStore;

    #[test]
    fn streamed_inputs_preserve_chunk_identity_and_refuse_incomplete_content() {
        let directory = tempfile::tempdir().unwrap();
        let store = DirStore::new(directory.path().join("products"));
        let bytes = vec![0x5a; MAX_RECORD + 1];
        // Deliberately short reads must produce the same canonical chunks.
        struct Short<'a>(&'a [u8]);
        impl std::io::Read for Short<'_> {
            fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
                let len = output.len().min(self.0.len()).min(17);
                output[..len].copy_from_slice(&self.0[..len]);
                self.0 = &self.0[len..];
                Ok(len)
            }
        }
        let reference = store_link_input(&store, &bytes).unwrap();
        assert_eq!(
            store_link_input_reader(&store, Short(&bytes)).unwrap(),
            reference
        );
        assert_eq!(link_input_bytes(&store, reference).unwrap(), bytes);
        let mut record: BlobRecord = get_record(&store, BLOB, reference).unwrap();
        record.len += 1;
        let invalid = put_record(&store, BLOB, &record).unwrap();
        assert!(link_input_bytes(&store, invalid).is_err());
        record.len -= 1;
        record.chunks[0].kind = BLOB.kind;
        let invalid = put_record(&store, BLOB, &record).unwrap();
        assert!(link_input_bytes(&store, invalid).is_err());
        let record: BlobRecord = get_record(&store, BLOB, reference).unwrap();
        let chunk: Reference = record.chunks[0].clone().into();
        let path = hirsel_artifact_store::Store::new(store.root().into()).output_path(chunk);
        std::fs::remove_file(path).unwrap();
        let reopened = DirStore::new(store.root().into());
        assert!(link_input_bytes(&reopened, reference).is_err());
        assert!(
            materialize_link_input(
                &reopened,
                reference,
                &directory.path().join("missing"),
                true
            )
            .is_err()
        );
        assert!(!directory.path().join("missing").exists());
    }

    #[test]
    fn materialization_restores_bytes_modes_and_durable_inputs_atomically() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let directory = tempfile::tempdir().unwrap();
        let store = DirStore::new(directory.path().join("products"));
        let reference = store_link_input_reader(&store, b"frozen linker input".as_slice()).unwrap();
        let path = directory.path().join("output/file");
        materialize_link_input(&store, reference, &path, true).unwrap();
        let inode = std::fs::metadata(&path).unwrap().ino();
        materialize_link_input(&store, reference, &path, true).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().ino(), inode);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        materialize_link_input(&store, reference, &path, true).unwrap();
        assert_ne!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o111,
            0
        );
        std::fs::write(&path, b"stale bytes").unwrap();
        let reopened = DirStore::new(store.root().into());
        materialize_link_input(&reopened, reference, &path, false).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"frozen linker input");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o111,
            0
        );
        std::fs::remove_file(&path).unwrap();
        materialize_link_input(&reopened, reference, &path, true).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"frozen linker input");
        let empty = store_link_input(&store, b"").unwrap();
        assert!(materialize_link_input(&store, empty, &path, true).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"frozen linker input");
    }
}
