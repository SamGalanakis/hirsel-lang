use hirsel_driver::native::object_queries::{
    link_input_bytes, materialize_link_input, store_link_input, store_link_input_reader,
};
use hirsel_engine::store::{DirStore, ResultStore};
use std::io::{self, Read};

struct Short<'a>(&'a [u8]);
impl Read for Short<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let n = bytes.len().min(self.0.len()).min(7919);
        bytes[..n].copy_from_slice(&self.0[..n]);
        self.0 = &self.0[n..];
        Ok(n)
    }
}
#[test]
fn frozen_link_streams_keep_slice_identity_across_chunk_boundaries_and_restore_exact_files() {
    let directory = tempfile::tempdir().unwrap();
    let store = DirStore::new(directory.path().join("products"));
    let bytes = (0..hirsel_artifact_store::MAX_RECORD + 17)
        .map(|n| (n.wrapping_mul(31) % 251) as u8)
        .collect::<Vec<_>>();
    let slice = store_link_input(&store, &bytes).unwrap();
    let stream = store_link_input_reader(&store, Short(&bytes)).unwrap();
    assert_eq!(
        stream, slice,
        "reader fragmentation cannot affect canonical blob identity"
    );
    assert_eq!(link_input_bytes(&store, stream).unwrap(), bytes);
    let path = directory.path().join("restored/input.a");
    materialize_link_input(&store, stream, &path, false).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    use std::os::unix::fs::MetadataExt;
    let inode = std::fs::metadata(&path).unwrap().ino();
    materialize_link_input(&store, stream, &path, false).unwrap();
    assert_eq!(std::fs::metadata(&path).unwrap().ino(), inode);
    std::fs::write(&path, b"stale tool bytes").unwrap();
    materialize_link_input(&store, stream, &path, true).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert_ne!(std::fs::metadata(&path).unwrap().mode() & 0o111, 0);
    let empty = store_link_input_reader(&store, io::empty()).unwrap();
    assert_eq!(empty, store_link_input(&store, &[]).unwrap());
    materialize_link_input(&store, empty, &path, false).unwrap();
    assert!(std::fs::read(&path).unwrap().is_empty());
    assert!(materialize_link_input(&store, empty, &path, true).is_err());
}
#[test]
fn frozen_link_streams_refuse_read_errors_and_missing_chunks_before_publishing_files() {
    let directory = tempfile::tempdir().unwrap();
    let store = DirStore::new(directory.path().join("products"));
    struct Broken;
    impl Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("source read failed"))
        }
    }
    assert!(
        store_link_input_reader(&store, Broken)
            .unwrap_err()
            .contains("source read failed")
    );
    let blob = store_link_input_reader(&store, io::Cursor::new(b"frozen link input")).unwrap();
    // Remove the selected bounded chunk while retaining its manifest.
    let chunk = hirsel_artifact_store::Domain {
        kind: 0x4f000005,
        schema: 1,
    }
    .reference(b"frozen link input")
    .unwrap();
    let path = directory.path().join("input");
    std::fs::write(&path, b"existing").unwrap();
    let outputs = hirsel_artifact_store::Store::new(store.root().into());
    std::fs::remove_file(outputs.output_path(chunk)).unwrap();
    assert!(store.get_content(blob).is_some());
    assert!(materialize_link_input(&store, blob, &path, false).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"existing");
}
