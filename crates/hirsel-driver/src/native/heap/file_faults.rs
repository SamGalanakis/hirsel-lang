//! Typed snapshot faults, ported from library/{dispatch,file,simulation}.rs and
//! io_sim::ScriptedFileFaults. Authority/readiness precede queue admission; the
//! ordinary Heap filesystem owns all identities, partial effects and durability.
use super::{Io, IoGrant, io_error, io_not_granted, io_portable, io_relative};
use hirsel_heapcheck::{OpOutcome, OpValue, Operations};
use hirsel_run_schema::{Access, FileFaultError as E, FileOperation as O, take_file_fault};

fn error(error: E) -> OpValue {
    match error {
        E::Fault(code) => io_error(code),
        E::Canceled(cause) => {
            let mut bytes = 1u32.to_le_bytes().to_vec();
            bytes.extend_from_slice(&cause.to_le_bytes());
            OpValue::Variant {
                tag: 1,
                value: Box::new(OpValue::Plain(bytes)),
            }
        }
        E::NotFound => io_portable(3),
        E::PermissionDenied => io_portable(4),
        E::AlreadyExists => io_portable(5),
        E::IsDirectory => io_portable(6),
        E::NotDirectory => io_portable(7),
        E::DirectoryNotEmpty => io_portable(8),
        E::CrossDevice => io_portable(9),
    }
}
impl Io {
    pub(super) fn scripted_file(&mut self, name: &str, args: &[OpValue]) -> Option<OpOutcome> {
        let op = match name {
            "open" => O::Open,
            "create" => O::Create,
            "open_append" => O::OpenAppend,
            "read" => O::Read,
            "read_bytes" => O::ReadBytes,
            "write" => O::Write,
            "append" => O::Append,
            "write_bytes" => O::WriteBytes,
            "create_atomic" => O::CreateAtomic,
            "write_all" => O::WriteAll,
            "commit" => O::Commit,
            "sync" => O::Sync,
            "put_new" => O::PutNew,
            _ => return None,
        };
        // Empty/exhausted scripts retain the ordinary provider's behavior.
        if self.file_faults.get(&op).is_none_or(|q| q.is_empty()) {
            return None;
        }
        let [
            OpValue::Handle(directory),
            OpValue::Handle(operand),
            rest @ ..,
        ] = args
        else {
            return Some(OpOutcome::Unsupported);
        };
        let Some(IoGrant::Dir { tree, root, access }) = self.grants.get(directory).cloned() else {
            return Some(OpOutcome::Fail("CAP-2".into()));
        };
        let writing = matches!(
            op,
            O::Create
                | O::OpenAppend
                | O::Write
                | O::Append
                | O::WriteBytes
                | O::CreateAtomic
                | O::WriteAll
                | O::Commit
                | O::PutNew
        );
        // Atomic arguments are checked even on a read-only directory.
        if matches!(op, O::WriteAll | O::Commit)
            && !matches!(self.grants.get(operand), Some(IoGrant::Atomic { tree: t, directory: d, .. }) if *t == tree && d == directory)
        {
            return Some(OpOutcome::Fail("CAP-2".into()));
        }
        if matches!(op, O::Read | O::Write | O::Append | O::Sync) {
            let Some(IoGrant::File {
                tree: t,
                directory: d,
                writable,
                path,
                ..
            }) = self.grants.get(operand)
            else {
                return Some(OpOutcome::Fail("CAP-2".into()));
            };
            if *t != tree || d != directory {
                return Some(OpOutcome::Fail("CAP-2".into()));
            }
            if writing && access == Access::Read {
                return Some(OpOutcome::Value(io_not_granted()));
            }
            if op == O::Read && *writable || matches!(op, O::Write | O::Append) && !*writable {
                return Some(OpOutcome::Value(io_error(1)));
            }
            if op == O::Read && !self.stale.contains(operand) && self.file_reads.blocked(tree, path)
            {
                return Some(OpOutcome::Pending);
            }
        } else {
            if writing && access == Access::Read {
                return Some(OpOutcome::Value(io_not_granted()));
            }
            if !matches!(op, O::WriteAll | O::Commit) {
                let text = self.paths.get(operand.checked_sub(1)? as usize)?;
                if io_relative(&root, text).is_none() {
                    return Some(OpOutcome::Value(io_error(1)));
                }
            }
        }
        let requested = match op {
            O::Read => match rest.get(1) {
                Some(OpValue::Bits { bits, .. }) => Some(*bits as u64),
                _ => return Some(OpOutcome::Unsupported),
            },
            O::Write | O::Append | O::WriteBytes | O::WriteAll | O::PutNew => match rest.first() {
                Some(OpValue::Bytes(bytes)) => Some(bytes.len() as u64),
                _ => return Some(OpOutcome::Unsupported),
            },
            O::Open
            | O::Create
            | O::OpenAppend
            | O::ReadBytes
            | O::CreateAtomic
            | O::Commit
            | O::Sync => None,
        };
        let step = match take_file_fault(&mut self.file_faults, op, requested) {
            Ok(Some(step)) => step,
            Ok(None) => return None,
            // Model refusal, never a language FileError or a consumed queue head.
            Err(_) => return Some(OpOutcome::Unsupported),
        };
        let fault = step.error?;
        // validate_file_faults forbids a prefix on reads, put_new or cancellation.
        if step.partial > 0 {
            let Some(OpValue::Bytes(bytes)) = rest.first() else {
                return Some(OpOutcome::Unsupported);
            };
            let prefix = &bytes[..step.partial as usize];
            match op {
                O::Write | O::Append => {
                    let Some(IoGrant::File {
                        identity, contents, ..
                    }) = self.grants.get(operand).cloned()
                    else {
                        return Some(OpOutcome::Unsupported);
                    };
                    self.append_file(tree, &identity, &contents, prefix);
                }
                O::WriteBytes | O::WriteAll => {
                    let mut partial = args.to_vec();
                    partial[2] = OpValue::Bytes(prefix.to_vec());
                    // Bypass admission: this is the effect of the consumed entry,
                    // not a second operation that may consume the next queue head.
                    let readiness = self.file_reads.clone();
                    let effect = self.io_ordinary(Some("file"), name, &partial);
                    // Failed writes cannot release an AfterWrite readiness gate.
                    self.file_reads = readiness;
                    match effect {
                        Some(OpOutcome::Value(_)) => {}
                        other => return Some(other.unwrap_or(OpOutcome::Unsupported)),
                    }
                }
                _ => return Some(OpOutcome::Unsupported),
            }
        }
        let value = error(fault);
        if op == O::Commit {
            if !matches!(self.close_handle(*operand, &mut Vec::new()), Ok(None)) {
                return Some(OpOutcome::Unsupported);
            }
            Some(OpOutcome::Complete {
                result: Ok(value),
                consumed: vec![1],
            })
        } else {
            Some(OpOutcome::Value(value))
        }
    }

    pub(super) fn append_file(
        &mut self,
        tree: u64,
        identity: &std::sync::Arc<()>,
        contents: &[u8],
        bytes: &[u8],
    ) {
        let path = self
            .files
            .iter()
            .find(|(_, id)| std::sync::Arc::ptr_eq(id, identity))
            .map(|((_, p), _)| p.clone());
        self.durable_write(tree, identity, contents.len(), bytes);
        if let Some(path) = path {
            let mut data = contents.to_vec();
            data.extend_from_slice(bytes);
            self.trees.get_mut(&tree).unwrap().1.insert(path, data);
        }
        self.update_files(identity, bytes, true);
    }
}

#[cfg(test)]
mod heap_file_faults_38 {
    use super::*;
    use hirsel_run_schema::{FileFault, FileReadScript};
    use std::collections::{BTreeMap, VecDeque};

    fn fixture() -> (Io, u64, u64) {
        let mut io = Io::new();
        let d = io.grant(IoGrant::Dir {
            tree: 0,
            root: String::new(),
            access: Access::ReadWrite,
        });
        let p = io.path("old");
        let identity = std::sync::Arc::new(());
        io.trees
            .get_mut(&0)
            .unwrap()
            .1
            .insert("old".into(), b"old".to_vec());
        io.files.insert((0, "old".into()), identity);
        (io, d, p)
    }
    fn queue(io: &mut Io, op: O, count: Option<u64>, error: Option<E>, partial: u64) {
        io.file_faults.insert(
            op,
            VecDeque::from([FileFault {
                requested: count,
                error,
                partial,
            }]),
        );
    }
    fn handle(outcome: Option<OpOutcome>) -> u64 {
        let Some(OpOutcome::Value(OpValue::Variant { tag: 0, value })) = outcome else {
            panic!("open failed");
        };
        let OpValue::OpenHandle(id) = *value else {
            panic!("not a handle");
        };
        id
    }
    fn call(io: &mut Io, name: &str, args: &[OpValue]) -> Option<OpOutcome> {
        io.io(Some("file"), name, args)
    }
    fn pair(d: u64, p: u64) -> [OpValue; 2] {
        [OpValue::Handle(d), OpValue::Handle(p)]
    }

    #[test]
    fn wrong_count_refuses_without_pop_or_effect() {
        let (mut io, d, p) = fixture();
        queue(&mut io, O::WriteBytes, Some(3), Some(E::Fault(7)), 2);
        let before = io.file_faults.clone();
        let args = [
            OpValue::Handle(d),
            OpValue::Handle(p),
            OpValue::Bytes(b"x".to_vec()),
        ];
        assert!(matches!(
            call(&mut io, "write_bytes", &args),
            Some(OpOutcome::Unsupported)
        ));
        assert_eq!(io.file_faults, before);
        assert_eq!(io.trees[&0].1["old"], b"old");
    }

    #[test]
    fn denied_and_invalid_paths_retain_queue_head() {
        let (mut io, d, p) = fixture();
        let read = io.grant(IoGrant::Dir {
            tree: 0,
            root: String::new(),
            access: Access::Read,
        });
        queue(&mut io, O::WriteBytes, Some(3), Some(E::Fault(7)), 2);
        let before = io.file_faults.clone();
        for (directory, path, tag) in [(read, p, 2), (d, io.path("../bad"), 0)] {
            let result = call(
                &mut io,
                "write_bytes",
                &[
                    OpValue::Handle(directory),
                    OpValue::Handle(path),
                    OpValue::Bytes(b"xyz".to_vec()),
                ],
            );
            let Some(OpOutcome::Value(OpValue::Variant { tag: 1, value })) = result else {
                panic!("expected refusal");
            };
            let OpValue::Plain(bytes) = *value else {
                panic!("expected FileError");
            };
            assert_eq!(u32::from_le_bytes(bytes[..4].try_into().unwrap()), tag);
            assert_eq!(io.file_faults, before);
        }
    }

    #[test]
    fn pending_cancel_release_and_wrong_mode_do_not_take_head() {
        let (mut io, d, p) = fixture();
        let f = handle(call(&mut io, "open", &pair(d, p)));
        queue(&mut io, O::Read, Some(1), Some(E::Fault(7)), 0);
        io.file_reads = hirsel_run_schema::FileReadiness::new(BTreeMap::from([(
            "old".into(),
            FileReadScript::AfterWrite {
                path: "ready".into(),
            },
        )]));
        let args = [
            OpValue::Handle(d),
            OpValue::Handle(f),
            OpValue::Bytes(b"prefix".to_vec()),
            OpValue::Bits {
                scalar: super::super::ScalarKind::U64,
                bits: 1,
            },
        ];
        let before = io.file_faults.clone();
        assert!(matches!(
            call(&mut io, "read", &args),
            Some(OpOutcome::Pending)
        ));
        assert!(matches!(
            io.cancel_file("read", &args, 0),
            Some(OpOutcome::Value(_))
        ));
        assert_eq!(io.file_faults, before);
        assert!(io.file_reads.wrote(0, "ready"));
        assert!(matches!(
            call(&mut io, "read", &args),
            Some(OpOutcome::Value(_))
        ));
        assert!(io.file_faults[&O::Read].is_empty());
        let Some(IoGrant::File { position, .. }) = io.grants.get(&f) else {
            panic!();
        };
        assert_eq!(*position, 0);
        io.close_handle(f, &mut vec![]).unwrap();
        assert!(!io.grants.contains_key(&f));
        let writable = handle(call(&mut io, "open_append", &pair(d, p)));
        queue(&mut io, O::Read, Some(1), Some(E::Fault(7)), 0);
        let mut wrong = args;
        wrong[1] = OpValue::Handle(writable);
        let before = io.file_faults.clone();
        assert!(matches!(
            call(&mut io, "read", &wrong),
            Some(OpOutcome::Value(_))
        ));
        assert_eq!(io.file_faults, before);
    }

    #[test]
    fn partial_failure_does_not_release_readiness_or_publish_atomic() {
        let (mut io, d, p) = fixture();
        io.file_reads = hirsel_run_schema::FileReadiness::new(BTreeMap::from([(
            "waiting".into(),
            FileReadScript::AfterWrite { path: "old".into() },
        )]));
        queue(&mut io, O::WriteBytes, Some(3), Some(E::Fault(7)), 2);
        call(
            &mut io,
            "write_bytes",
            &[
                OpValue::Handle(d),
                OpValue::Handle(p),
                OpValue::Bytes(b"xyz".to_vec()),
            ],
        );
        assert_eq!(io.trees[&0].1["old"], b"xy");
        assert!(io.file_reads.blocked(0, "waiting"));
        let f = handle(call(&mut io, "create_atomic", &pair(d, p)));
        queue(&mut io, O::WriteAll, Some(3), Some(E::Fault(7)), 2);
        call(
            &mut io,
            "write_all",
            &[
                OpValue::Handle(d),
                OpValue::Handle(f),
                OpValue::Bytes(b"new".to_vec()),
            ],
        );
        assert_eq!(io.trees[&0].1["old"], b"xy");
        assert_eq!(io.trees[&0].1.len(), 2);
        queue(&mut io, O::Commit, None, Some(E::PermissionDenied), 0);
        assert!(
            matches!(call(&mut io, "commit", &pair(d, f)), Some(OpOutcome::Complete { consumed, .. }) if consumed == vec![1])
        );
        assert_eq!(
            io.trees[&0].1,
            BTreeMap::from([("old".into(), b"xy".to_vec())])
        );
        assert!(!io.grants.contains_key(&f));
    }

    #[test]
    fn sync_failure_keeps_previous_durable_bytes() {
        let (mut io, d, p) = fixture();
        io.durable_start(0);
        let f = handle(call(&mut io, "open_append", &pair(d, p)));
        call(
            &mut io,
            "append",
            &[
                OpValue::Handle(d),
                OpValue::Handle(f),
                OpValue::Bytes(b"new".to_vec()),
            ],
        );
        queue(&mut io, O::Sync, None, Some(E::Fault(17)), 0);
        assert!(matches!(
            call(&mut io, "sync", &pair(d, f)),
            Some(OpOutcome::Value(_))
        ));
        let seed = (0..100)
            .find(|seed| {
                let b = hirsel_rt::io::RandomSource::SplitMix64(*seed)
                    .draw(8)
                    .unwrap();
                u64::from_le_bytes(b.try_into().unwrap()) % 4 == 0
            })
            .unwrap();
        io.crash(d, seed).unwrap();
        assert_eq!(io.trees[&0].1["old"], b"old");
    }

    #[test]
    fn complete_script_validated_before_launch_even_unused_tail() {
        let world = super::super::HeapWorld {
            file_faults: BTreeMap::from([(
                O::Write,
                VecDeque::from([
                    FileFault {
                        requested: Some(3),
                        error: None,
                        partial: 0,
                    },
                    FileFault {
                        requested: Some(3),
                        error: Some(E::Canceled(0)),
                        partial: 1,
                    },
                ]),
            )]),
            ..Default::default()
        };
        let result = Io::for_launch(&super::super::super::link::LaunchKind::Zero, 0, 0, &world);
        assert!(
            result
                .err()
                .unwrap()
                .to_string()
                .contains("unsupported partial file fault")
        );
    }
    #[test]
    fn live_cancellation_ignores_scripted_partial_prefix() {
        let (mut io, d, p) = fixture();
        queue(&mut io, O::WriteBytes, Some(3), Some(E::Fault(7)), 2);
        let before = io.file_faults.clone();
        let args = [
            OpValue::Handle(d),
            OpValue::Handle(p),
            OpValue::Bytes(b"xyz".to_vec()),
        ];
        assert!(matches!(
            io.cancel_file("write_bytes", &args, 1),
            Some(OpOutcome::Value(_))
        ));
        assert_eq!(io.file_faults, before);
        assert_eq!(io.trees[&0].1["old"], b"old");
    }
}
