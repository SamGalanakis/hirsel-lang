use crate::support::filesystem::*;
use crate::support::library::{Io, Library};
use hirsel_rt::{io::*, io_real::RealIo, io_sim::SimIo};
use std::collections::BTreeMap;
use std::path::Path;
#[test]
fn replay_rejects_missing_extra_reordered_and_forged_completions_without_effects() {
    let (_temp, real) = setup();
    let mut real = Library(real);
    actions(&mut real);
    let tape = real.0.tape().to_vec();
    let mut empty = SimIo::new(initial(), vec![]);
    assert_eq!(
        empty.call(tape[0].request.clone()),
        Err(AdapterError::MissingInput(0))
    );
    let mut sim = SimIo::new(initial(), tape.clone());
    assert_eq!(sim.finish(), Err(AdapterError::UnconsumedInput(0)));
    assert_eq!(
        sim.call(tape[1].request.clone()),
        Err(AdapterError::ReplayMismatch(0))
    );
    assert_eq!(sim.position(), 0);
    assert_eq!(sim.tree(), &initial());
    for event in &tape {
        sim.call(event.request.clone()).unwrap();
    }
    assert_eq!(
        sim.call(tape[0].request.clone()),
        Err(AdapterError::MissingInput(tape.len()))
    );
    let mut forged = tape.clone();
    forged[0].completion = Completion::Handle(Ok(99));
    let mut sim = SimIo::new(initial(), forged);
    assert_eq!(
        sim.call(tape[0].request.clone()),
        Err(AdapterError::ReplayMismatch(0))
    );
    assert_eq!(sim.handles().observed().len(), 1);
    let idx = tape
        .iter()
        .position(|e| matches!(&e.request, Request::Write { .. }))
        .unwrap();
    let mut forged = tape.clone();
    forged[idx].completion = Completion::Unit(Ok(()));
    let mut sim = SimIo::new(initial(), forged);
    for e in &tape[..idx] {
        sim.call(e.request.clone()).unwrap();
    }
    let before = sim.tree().clone();
    assert_eq!(
        sim.call(tape[idx].request.clone()),
        Err(AdapterError::ReplayMismatch(idx))
    );
    assert_eq!(sim.tree(), &before);
}
#[test]
fn confinement_refuses_escaping_links_and_follows_in_root_links() {
    use std::os::unix::fs::symlink;
    let (temp, real) = setup();
    let outside = Disposable::new();
    std::fs::write(outside.path().join("secret"), b"private").unwrap();
    symlink(outside.path(), temp.path().join("escape")).unwrap();
    symlink("../old", temp.path().join("sub/sibling")).unwrap();
    std::fs::create_dir(temp.path().join("sub/middle")).unwrap();
    symlink(outside.path(), temp.path().join("sub/middle/link")).unwrap();
    symlink(outside.path().join("secret"), temp.path().join("sub/last")).unwrap();
    symlink("sub", temp.path().join("dir-link")).unwrap();
    let mut real = Library(real);
    let root = real.directory(Io(1), "").unwrap().unwrap();
    let sub = real.narrow_dir(root, "sub").unwrap().unwrap();
    for path in ["/etc/passwd", "../old", "sibling", "../../secret"] {
        assert!(real.read(sub, path).unwrap().is_err(), "{path}");
    }
    for path in ["escape/secret", "sub/middle/link/secret", "sub/last"] {
        assert!(real.read(root, path).unwrap().is_err(), "{path}");
    }
    assert_eq!(real.read(root, "dir-link/a"), Ok(Ok(b"A".to_vec())));
    let linked = real.narrow_dir(root, "dir-link").unwrap().unwrap();
    assert_eq!(real.read(linked, "a"), Ok(Ok(b"A".to_vec())));
    assert!(real.read(linked, "sibling").unwrap().is_err());
    assert_eq!(real.read(sub, "é"), Ok(Ok(vec![0, 0xff])));
    assert_eq!(
        std::fs::read(outside.path().join("secret")).unwrap(),
        b"private"
    );
}
#[test]
fn repeated_directory_grants_keep_distinct_inodes_and_narrow_refuses_a_replaced_file() {
    let (temp, real) = setup();
    let mut real = Library(real);
    let root = real.directory(Io(1), "").unwrap().unwrap();
    let old_sub = real.narrow_dir(root, "sub").unwrap().unwrap();
    let file = real.open(old_sub, "a").unwrap().unwrap();
    std::fs::rename(temp.path().join("sub"), temp.path().join("moved")).unwrap();
    std::fs::create_dir(temp.path().join("sub")).unwrap();
    std::fs::write(temp.path().join("sub/a"), b"replacement").unwrap();
    let new_sub = real.narrow_dir(root, "sub").unwrap().unwrap();
    assert_eq!(real.read(old_sub, "a"), Ok(Ok(b"A".to_vec())));
    assert_eq!(real.read(new_sub, "a"), Ok(Ok(b"replacement".to_vec())));
    assert_eq!(real.narrow_file(new_sub, file), Ok(None));
}

use crate::support;
#[test]
fn io_result_records_match_derived_layouts_on_both_targets() {
    use hirsel_layout::layout::{LayoutBuilder, OwnerRepresentation, Owners};

    use hirsel_lir::layout_types::{SizeSpec, TargetSpec};
    use hirsel_types::Primitive;
    use std::mem::{align_of, offset_of, size_of};
    for target in [
        TargetSpec::x86_64_unknown_linux_gnu(),
        TargetSpec::aarch64_unknown_linux_gnu(),
    ] {
        let mut ty = support::schema::ty();
        let u32ty = ty.p(Primitive::U32);
        let u64ty = ty.p(Primitive::U64);
        let cancel = ty.variant(&[&[], &[], &[]]);
        let error = ty.variant(&[&[u32ty], &[cancel]]);
        let file = ty.named(&support::schema::path("File"), Vec::new());
        let mut owners = Owners::default();
        owners.nominals.insert(
            hirsel_syntax::DefId::intern(&support::schema::path("File")),
            OwnerRepresentation::Handle {
                resource: true,
                revision: Default::default(),
            },
        );
        let result = ty.variant(&[&[file], &[error]]);
        let size_result = ty.variant(&[&[u64ty], &[error]]);
        let option = ty.variant(&[&[], &[file]]);
        let unit = ty.variant(&[&[], &[error]]);
        let bytes = ty.p(Primitive::Bytes);
        let bytes_result = ty.variant(&[&[bytes], &[error]]);
        let path = ty.named(
            &support::schema::handle(&mut owners, "Path", false),
            Vec::new(),
        );
        let item = ty.variant(&[&[path], &[error]]);
        let iter = ty.add(hirsel_types::TypeKind::Builtin {
            kind: hirsel_types::BuiltinType::Iter,
            arguments: vec![hirsel_types::GenericArgument::Type(item)].into(),
        });
        let walk = ty.variant(&[&[iter], &[error]]);
        let step = ty.variant(&[&[], &[item]]);
        let kind = ty.variant(&[&[], &[], &[], &[]]);
        let i128ty = ty.p(Primitive::I128);
        let timestamp = ty.variant(&[&[], &[i128ty]]);
        let permissions = ty.p(Primitive::U32);
        let metadata = ty.record(&[kind, u64ty, timestamp, permissions]);
        let metadata_result = ty.variant(&[&[metadata], &[error]]);
        let mut builder = LayoutBuilder::new(&target, &owners);
        let metadata_id = builder.layout(&ty.store, metadata_result).unwrap();
        let ids: Vec<_> = [result, size_result, option, unit, bytes_result, walk, step]
            .iter()
            .map(|&t| builder.layout(&ty.store, t).unwrap())
            .collect();
        let table = builder.finish();
        hirsel_lir::layout_validate::validate_table(&table, &target).unwrap();
        let meta_layout = table.layout(metadata_id).unwrap();
        assert_eq!(
            meta_layout.size,
            SizeSpec::Fixed(size_of::<MetadataResult>() as u64)
        );
        assert_eq!(meta_layout.align, align_of::<MetadataResult>() as u32);
        assert_eq!(
            meta_layout.tag.as_ref().unwrap().variants[0].fields[0].offset,
            16
        );
        for &id in &ids[..3] {
            let record = table.layout(id).unwrap();
            assert_eq!(
                record.size,
                SizeSpec::Fixed(size_of::<HandleResult>() as u64)
            );
            assert_eq!(record.align, align_of::<HandleResult>() as u32);
            assert_eq!(
                record.tag.as_ref().unwrap().variants[1].fields[0].offset,
                offset_of!(HandleResult, value) as u64
            );
        }
        let record = table.layout(ids[3]).unwrap();
        assert_eq!(record.size, SizeSpec::Fixed(size_of::<UnitResult>() as u64));
        assert_eq!(record.align, align_of::<UnitResult>() as u32);
        assert_eq!(
            record.tag.as_ref().unwrap().variants[1].fields[0].offset,
            offset_of!(UnitResult, error) as u64
        );
        for (id, size, align, payload) in [
            (
                ids[4],
                size_of::<BytesResult>(),
                align_of::<BytesResult>(),
                offset_of!(BytesResult, value),
            ),
            (
                ids[5],
                size_of::<WalkResult>(),
                align_of::<WalkResult>(),
                offset_of!(WalkResult, value),
            ),
            (
                ids[6],
                size_of::<WalkOption>(),
                align_of::<WalkOption>(),
                offset_of!(WalkOption, value),
            ),
        ] {
            let record = table.layout(id).unwrap();
            assert_eq!(record.size, SizeSpec::Fixed(size as u64));
            assert_eq!(record.align, align as u32);
            assert_eq!(
                record.tag.as_ref().unwrap().variants[if id == ids[6] { 1 } else { 0 }].fields[0]
                    .offset,
                payload as u64
            );
        }
    }
}

/// The runtime images of variants whose `None` is a null String or Bytes owner word
/// (layout version 5) are exactly the derived layouts, niche offset included, and so is the
/// TlsClient record holding a niche-encoded `Option[TlsIdentity]`.
#[test]
fn niche_images_match_derived_layouts_on_both_targets() {
    use hirsel_layout::layout::{LayoutBuilder, Owners};
    use hirsel_lir::layout_types::{SizeSpec, TagForm, TargetSpec};
    use hirsel_rt::net_abi::NetErrorAbi;
    use hirsel_rt::tls_abi::{NetChainResult, TlsClientAbi, TlsRootsAbi};
    use hirsel_types::{Primitive, TypeId};
    use std::mem::{align_of, offset_of, size_of};
    for target in [
        TargetSpec::x86_64_unknown_linux_gnu(),
        TargetSpec::aarch64_unknown_linux_gnu(),
    ] {
        let mut ty = support::schema::ty();
        let (u8ty, u32ty) = (ty.p(Primitive::U8), ty.p(Primitive::U32));
        let (string, bytes) = (ty.p(Primitive::String), ty.p(Primitive::Bytes));
        let env_error = ty.variant(&[&[], &[]]);
        let cancel = ty.variant(&[&[], &[], &[]]);
        let mut tls: Vec<&[TypeId]> = vec![&[]; 10];
        let alert = [u8ty];
        tls[6] = &alert;
        let tls_failure = ty.variant(&tls);
        let mut net: Vec<&[TypeId]> = vec![&[]; 18];
        let (canceled, failed, fault) = ([cancel], [tls_failure], [u32ty]);
        (net[1], net[16], net[17]) = (&canceled, &failed, &fault);
        let net_error = ty.variant(&net);
        let maybe_string = ty.variant(&[&[], &[string]]);
        let maybe_bytes = ty.variant(&[&[], &[bytes]]);
        let env = ty.variant(&[&[maybe_string], &[env_error]]);
        let chain = ty.variant(&[&[maybe_bytes], &[net_error]]);
        let roots = ty.variant(&[&[], &[bytes]]);
        let identity = ty.record(&[bytes, bytes]);
        let maybe_identity = ty.variant(&[&[], &[identity]]);
        let alpn = ty.list(string);
        let client = ty.record(&[roots, maybe_identity, alpn]);
        let owners = Owners::default();
        let mut builder = LayoutBuilder::new(&target, &owners);
        let ids =
            [net_error, env, chain, roots, client].map(|t| builder.layout(&ty.store, t).unwrap());
        let table = builder.finish();
        hirsel_lir::layout_validate::validate_table(&table, &target).unwrap();
        let record = |id| table.layout(id).unwrap();
        let none_is_null = TagForm::Niche {
            nullary: 0,
            dataful: 1,
        };
        let error = record(ids[0]);
        assert_eq!(
            (error.size.clone(), error.align),
            (
                SizeSpec::Fixed(size_of::<NetErrorAbi>() as u64),
                align_of::<NetErrorAbi>() as u32
            )
        );
        for (id, size, align, value) in [
            (
                ids[1],
                size_of::<EnvResult>(),
                align_of::<EnvResult>(),
                offset_of!(EnvResult, value),
            ),
            (
                ids[2],
                size_of::<NetChainResult>(),
                align_of::<NetChainResult>(),
                offset_of!(NetChainResult, value),
            ),
        ] {
            let result = record(id);
            assert_eq!(result.size, SizeSpec::Fixed(size as u64));
            assert_eq!(result.align, align as u32);
            let arms = &result.tag.as_ref().unwrap().variants;
            assert_eq!(arms[0].fields[0].offset, value as u64);
            assert_eq!(arms[1].fields[0].offset, value as u64);
            let option = record(arms[0].fields[0].layout).tag.as_ref().unwrap();
            assert_eq!((option.offset, option.form), (0, none_is_null));
        }
        let roots = record(ids[3]);
        assert_eq!(
            (roots.size.clone(), roots.align),
            (
                SizeSpec::Fixed(size_of::<TlsRootsAbi>() as u64),
                align_of::<TlsRootsAbi>() as u32
            )
        );
        let tag = roots.tag.as_ref().unwrap();
        assert_eq!(
            (tag.offset, tag.form, tag.variants[1].fields[0].offset),
            (0, none_is_null, offset_of!(TlsRootsAbi, pem) as u64)
        );
        let client = record(ids[4]);
        assert_eq!(
            (client.size.clone(), client.align),
            (
                SizeSpec::Fixed(size_of::<TlsClientAbi>() as u64),
                align_of::<TlsClientAbi>() as u32
            )
        );
        let offsets: Vec<u64> = client.fields.iter().map(|field| field.offset).collect();
        assert_eq!(
            offsets,
            [
                offset_of!(TlsClientAbi, roots),
                offset_of!(TlsClientAbi, identity),
                offset_of!(TlsClientAbi, alpn)
            ]
            .map(|offset| offset as u64)
        );
        let option = record(client.fields[1].layout).tag.as_ref().unwrap();
        assert_eq!((option.offset, option.form), (0, none_is_null));
    }
    // The exports writing the chain image declare its niche size to the emitter.
    for symbol in [
        "hirsel_rt_v2_tls_peer",
        "hirsel_rt_v2_tls_negotiated_protocol",
    ] {
        let export = hirsel_rt_operations::HIRSEL_ABI_EXPORTS
            .iter()
            .find(|export| export.symbol == symbol)
            .unwrap();
        assert_eq!(
            export.result,
            Some((
                size_of::<NetChainResult>() as u64,
                align_of::<NetChainResult>() as u32
            )),
            "{symbol}"
        );
    }
}

#[test]
fn launch_recording_round_trips_inputs_empty_fields_and_faults_and_refuses_malformed_data() {
    use hirsel_rt::launch::{
        Recording, parse_recording, parse_tape, render_recording, render_tape,
    };
    let (_temp, real) = setup();
    let mut real = Library(real);
    actions(&mut real);
    let mut events = real.0.tape().to_vec();
    events.push(Event {
        request: Request::MonotonicClock { io: 1 },
        completion: Completion::ClockGrant(Ok(20)),
        fault: None,
    });
    events.push(Event {
        request: Request::Random { io: 1 },
        completion: Completion::Handle(Ok(21)),
        fault: None,
    });
    events.push(Event {
        request: Request::TestDir {
            test: 22,
            entries: vec![("".into(), vec![]), ("é".into(), vec![0, 255])],
        },
        completion: Completion::Handle(Ok(23)),
        fault: None,
    });
    events.push(Event {
        request: Request::TestCheck {
            test: 22,
            condition: false,
        },
        completion: Completion::Checked(false),
        fault: None,
    });
    let text = render_tape(&events).unwrap();
    assert_eq!(parse_tape(&text).unwrap(), events);
    let record = Recording {
        arguments: vec!["".into(), "é/x".into()],
        run_identity: [0; 32],
        hash_seed: 0,
        random_seed: 0,
        test_seed: u64::MAX,
        binding: None,
        tree: initial(),
        events,
    };
    let text = render_recording(&record).unwrap();
    assert_eq!(parse_recording(&text).unwrap(), record);
    for malformed in [
        text.replace("test_seed 18446744073709551615", "test_seed x"),
        text.replace("events\n", ""),
        text.replace("directory \n", "directory 2e2e\n"),
        text.replace("argument c3a92f78", "argument ff"),
        text.replace("test_seed 18446744073709551615", "test_seed 1\ntest_seed 2"),
    ] {
        assert!(parse_recording(&malformed).is_err());
    }
}

#[test]
fn path_order_uses_exact_text_without_allocating_or_comparing_capability_ids() {
    let ctx = support::Ctx::diagnostic();
    let mut binding = Box::new(Binding::new(SimIo::new(
        Tree::new(BTreeMap::new()).unwrap(),
        vec![],
    )));
    let z = binding.intern_path("z");
    let a = binding.intern_path("a");
    let unicode = binding.intern_path("é");
    let decomposed = binding.intern_path("e\u{301}");
    assert!(a > z);
    assert_eq!(binding.intern_path("a"), a);
    assert_ne!(unicode, decomposed);
    unsafe { binding.bind(&mut *ctx.0) };
    let allocations = support::host_calls();
    unsafe {
        assert_eq!(hirsel_rt_v2_path_compare(ctx.0, a, z), -1);
        assert_eq!(hirsel_rt_v2_path_compare(ctx.0, z, a), 1);
        assert_eq!(hirsel_rt_v2_path_compare(ctx.0, a, a), 0);
        assert_eq!(hirsel_rt_v2_path_compare(ctx.0, decomposed, unicode), -1);
        (*ctx.0).bind_io_data(std::ptr::null_mut());
    }
    assert_eq!(support::host_calls(), allocations);
}

#[test]
fn walk_yields_link_cycles_without_descending_or_resolving_targets() {
    use std::os::unix::fs::symlink;
    let (temp, real) = setup();
    let mut real = Library(real);
    let root = real.directory(Io(1), "").unwrap().unwrap();
    symlink("second", temp.path().join("first")).unwrap();
    symlink("first", temp.path().join("second")).unwrap();
    assert_eq!(
        real.walk(root),
        Ok(Ok(["first", "old", "second", "sub/a", "sub/é"]
            .map(str::to_owned)
            .to_vec()))
    );
    assert_eq!(real.read(root, "first"), Ok(Err(IoError::Fault(1))));
    std::fs::remove_file(temp.path().join("first")).unwrap();
    std::fs::remove_file(temp.path().join("second")).unwrap();
    symlink("..", temp.path().join("sub/back")).unwrap();
    assert_eq!(
        real.walk(root),
        Ok(Ok(["old", "sub/a", "sub/back", "sub/é"]
            .map(str::to_owned)
            .to_vec()))
    );
    std::fs::remove_file(temp.path().join("sub/back")).unwrap();
    // A cycle through sibling directories must preserve traversal ancestry
    // independently of the descriptor ancestry used to resolve each link.
    std::fs::create_dir(temp.path().join("other")).unwrap();
    symlink("../other", temp.path().join("sub/across")).unwrap();
    symlink("../sub", temp.path().join("other/across")).unwrap();
    assert_eq!(
        real.walk(root),
        Ok(Ok(["old", "other/across", "sub/a", "sub/across", "sub/é"]
            .map(str::to_owned)
            .to_vec()))
    );
    std::fs::remove_file(temp.path().join("sub/across")).unwrap();
    std::fs::remove_file(temp.path().join("other/across")).unwrap();
    assert!(real.walk(root).unwrap().is_ok());
}

#[test]
fn fifo_and_socket_entries_are_refused_before_data_open() {
    use std::ffi::CString;
    use std::os::unix::net::UnixListener;
    let (temp, real) = setup();
    let fifo = CString::new(temp.path().join("fifo").as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: terminated disposable path; mkfifo creates no open descriptor.
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    // Watching IN_OPEN proves that refusal precedes a FIFO data open. Merely
    // using O_NONBLOCK would terminate too, but would emit this event.
    use std::os::fd::FromRawFd;
    let monitor = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
    assert!(monitor >= 0);
    let watch = unsafe { libc::inotify_add_watch(monitor, fifo.as_ptr(), libc::IN_OPEN) };
    assert!(watch >= 0);
    let mut monitor = unsafe { std::fs::File::from_raw_fd(monitor) };
    let mut real = Library(real);
    let root = real.directory(Io(1), "").unwrap().unwrap();
    assert_eq!(real.walk(root), Ok(Err(IoError::Fault(1))));
    assert!(real.read(root, "fifo").unwrap().is_err());
    assert_eq!(
        std::io::Read::read(&mut monitor, &mut [0; 1024])
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::WouldBlock
    );
    std::fs::remove_file(temp.path().join("fifo")).unwrap();
    let _socket = UnixListener::bind(temp.path().join("socket")).unwrap();
    assert_eq!(real.walk(root), Ok(Err(IoError::Fault(1))));
    std::fs::remove_file(temp.path().join("socket")).unwrap();
    assert!(real.walk(root).unwrap().is_ok());
}

#[test]
fn absolute_link_targets_are_refused_even_when_they_point_inside_the_grant() {
    use std::os::unix::fs::symlink;
    let (temp, real) = setup();
    let mut real = Library(real);
    let root = real.directory(Io(1), "").unwrap().unwrap();
    symlink(temp.path().join("old"), temp.path().join("link")).unwrap();
    assert_eq!(real.read(root, "link"), Ok(Err(IoError::Fault(1))));
    assert_eq!(
        real.walk(root),
        Ok(Ok(["link", "old", "sub/a", "sub/é"]
            .map(str::to_owned)
            .to_vec()))
    );
    std::fs::remove_file(temp.path().join("link")).unwrap();
    symlink("old", temp.path().join("link")).unwrap();
    assert_eq!(real.read(root, "link"), Ok(Ok(b"old".to_vec())));
    assert!(real.walk(root).unwrap().is_ok());
}

#[test]
fn commit_refuses_a_replaced_temporary_with_a_regular_prepared_twin() {
    use std::os::unix::fs::symlink;
    let (temp, io) = setup();
    let mut real = Library(io);
    let root = real.directory(Io(1), "").unwrap().unwrap();
    let mut atomic = real.create_atomic(root, "saved").unwrap().unwrap();
    real.write_all(root, &mut atomic, b"prepared")
        .unwrap()
        .unwrap();
    std::fs::remove_file(temp.path().join("saved.hirsel-tmp-3")).unwrap();
    symlink("old", temp.path().join("saved.hirsel-tmp-3")).unwrap();
    assert_eq!(real.commit(root, atomic), Ok(Err(IoError::Fault(1))));
    assert!(!temp.path().join("saved").exists());
    assert_eq!(std::fs::read(temp.path().join("old")).unwrap(), b"old");
    let mut atomic = real.create_atomic(root, "saved").unwrap().unwrap();
    real.write_all(root, &mut atomic, b"prepared")
        .unwrap()
        .unwrap();
    real.commit(root, atomic).unwrap().unwrap();
    assert_eq!(
        std::fs::read(temp.path().join("saved")).unwrap(),
        b"prepared"
    );
}

#[test]
fn fake_test_directories_derive_identity_and_refuse_foreign_trees() {
    use crate::support::library::{Dir, File};
    let (temp, mut io) = setup();
    io.test_context(7);
    let mut real = Library(io);
    let make = |io: &mut RealIo| match io
        .call(Request::TestDir {
            test: 1,
            entries: vec![("sub/value".into(), b"data".to_vec())],
        })
        .unwrap()
    {
        Completion::Handle(Ok(id)) => Dir(id),
        other => panic!("{other:?}"),
    };
    let dir = make(&mut real.0);
    let foreign = make(&mut real.0);
    let sub = real.narrow_dir(dir, "sub").unwrap().unwrap();
    let file = real.open(dir, "sub/value").unwrap().unwrap();
    assert_eq!(real.narrow_file(foreign, file), Ok(None));
    let old = real.open(dir, "sub/value").unwrap().unwrap();
    let old = real.narrow_file(sub, old).unwrap().unwrap();
    real.write(dir, "sub/value", b"data".to_vec())
        .unwrap()
        .unwrap();
    assert_eq!(real.narrow_file(sub, old), Ok(None));
    let old = real.open(dir, "sub/value").unwrap().unwrap();
    real.remove(sub, "value").unwrap().unwrap();
    real.write(sub, "value", b"data".to_vec()).unwrap().unwrap();
    assert_eq!(real.narrow_file(dir, old), Ok(None));
    let old = real.open(dir, "sub/value").unwrap().unwrap();
    let mut atomic = real.create_atomic(sub, "value").unwrap().unwrap();
    real.write_all(sub, &mut atomic, b"data").unwrap().unwrap();
    real.commit(sub, atomic).unwrap().unwrap();
    assert_eq!(real.narrow_file(sub, old), Ok(None));
    let fresh = real.open(sub, "value").unwrap().unwrap();
    let File(id) = real.narrow_file(sub, fresh).unwrap().unwrap();
    real.close_file(File(id)).unwrap().unwrap();
    assert!(!temp.path().join("value").exists());
    let tape = identity_tape(real.0.tape());
    let mut sim = SimIo::new(initial(), tape.clone());
    sim.test_context(7);
    for event in &tape {
        assert_eq!(
            sim.call(event.request.clone()),
            Ok(event.completion.clone())
        );
    }
    sim.finish().unwrap();
    assert_eq!(sim.handles(), real.0.handles());
    assert_eq!(sim.fake_trees(), real.0.fake_trees());
}

#[test]
fn replay_rejects_forged_success_for_a_stale_identity_without_consuming_its_handle() {
    let (_temp, io) = setup();
    let mut real = Library(io);
    let dir = real.directory(Io(1), ".").unwrap().unwrap();
    let file = real.open(dir, "old").unwrap().unwrap();
    real.write(dir, "old", b"old".to_vec()).unwrap().unwrap();
    assert_eq!(real.narrow_file(dir, file), Ok(None));
    let mut tape = identity_tape(real.0.tape());
    let last = tape.last_mut().unwrap();
    last.completion = Completion::OptionalFile(Some(99));
    let mut sim = SimIo::new(initial(), tape.clone());
    for event in &tape[..tape.len() - 1] {
        sim.call(event.request.clone()).unwrap();
    }
    let before = sim.handles().clone();
    let event = tape.last().unwrap();
    for _ in 0..2 {
        assert_eq!(
            sim.call(event.request.clone()),
            Err(AdapterError::ReplayMismatch(tape.len() - 1))
        );
        assert_eq!(sim.position(), tape.len() - 1);
        assert_eq!(sim.handles(), &before);
        assert_eq!(sim.tree(), &initial());
    }
}

#[test]
fn projection_denial_is_a_recorded_result_and_replay_cannot_mint_a_grant() {
    use std::collections::BTreeSet;
    for request in [
        Request::Stdout { io: 1 },
        Request::Tasks { io: 1 },
        Request::Net { io: 1 },
        Request::Random { io: 1 },
        Request::Directory {
            io: 1,
            path: "".into(),
        },
    ] {
        let (temp, mut real) = setup();
        real.set_projections(BTreeSet::new());
        let before = real.handles().clone();
        assert_eq!(
            real.call(request.clone()),
            Ok(Completion::Handle(Err(IoError::NotGranted)))
        );
        assert_eq!(real.handles(), &before);
        assert!(real.stdout().is_empty());
        let tape = real.tape().to_vec();
        let mut replay = SimIo::new(initial(), tape.clone());
        replay.set_projections(BTreeSet::new());
        assert_eq!(
            replay.call(request.clone()),
            Ok(Completion::Handle(Err(IoError::NotGranted)))
        );
        replay.finish().unwrap();
        assert_eq!(replay.handles(), &before);
        let mut forged = tape;
        forged[0].completion = Completion::Handle(Ok(2));
        let mut replay = SimIo::new(initial(), forged);
        replay.set_projections(BTreeSet::new());
        assert_eq!(
            replay.call(request.clone()),
            Err(AdapterError::ReplayMismatch(0))
        );
        assert_eq!(replay.position(), 0);
        assert_eq!(replay.handles(), &before);
        let mut granted = RealIo::grant(temp.path()).unwrap();
        assert!(matches!(
            granted.call(request),
            Ok(Completion::Handle(Ok(_)))
        ));
    }
}

#[test]
fn grant_and_file_refusals_have_the_declared_inline_layouts() {
    use std::mem::{align_of, offset_of, size_of};
    assert_eq!(
        (size_of::<GrantResult>(), align_of::<GrantResult>()),
        (16, 8)
    );
    assert_eq!(offset_of!(GrantResult, value), 8);
    assert_eq!(
        GrantResult::of(Err(IoError::NotGranted)),
        GrantResult {
            tag: 1,
            padding: 0,
            value: 0
        }
    );
    assert_eq!(
        GrantResult::of(Ok(17)),
        GrantResult {
            tag: 0,
            padding: 0,
            value: 17
        }
    );
    assert_eq!(
        HandleResult::of(Err(IoError::NotGranted)),
        HandleResult {
            tag: 1,
            padding: 0,
            value: 2
        }
    );
    assert_eq!(
        UnitResult::of(Err(IoError::NotGranted)),
        UnitResult {
            tag: 1,
            error: 2,
            code: 0
        }
    );
}
#[test]
fn host_input_tape_rechecks_exact_grants_and_never_reads_the_provider_on_replay() {
    use std::cell::Cell;
    use std::rc::Rc;
    #[derive(Clone)]
    struct Input {
        calls: Rc<Cell<usize>>,
        bytes: std::io::Cursor<Vec<u8>>,
    }
    impl std::io::Read for Input {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            self.calls.set(self.calls.get() + 1);
            std::io::Read::read(&mut self.bytes, out)
        }
    }
    let calls = Rc::new(Cell::new(0));
    let mut real = RealIo::from_pinned_mounts([], std::io::sink()).unwrap();
    real.set_stdin(Input {
        calls: calls.clone(),
        bytes: std::io::Cursor::new(vec![0, 255, 10]),
    });
    real.set_env(BTreeMap::from([
        ("SET".into(), Some("λ".into())),
        ("UNSET".into(), None),
    ]));
    real.set_projections(Default::default());
    assert_eq!(
        real.call(Request::Stdin { io: 1 }),
        Ok(Completion::Handle(Err(IoError::NotGranted)))
    );
    assert_eq!(calls.get(), 0);
    real.set_projections(["stdin".into(), "stderr".into()].into_iter().collect());
    let Completion::Handle(Ok(stdin)) = real.call(Request::Stdin { io: 1 }).unwrap() else {
        panic!()
    };
    real.inject(Fault {
        error: IoError::Canceled(1),
        partial: 0,
    });
    assert_eq!(
        real.call(Request::ReadStdin { stdin, max: 0 }),
        Ok(Completion::Input(Err(InputError::InvalidInput)))
    );
    assert_eq!(calls.get(), 0);
    assert_eq!(
        real.call(Request::ReadStdin { stdin: 999, max: 2 }),
        Err(AdapterError::InvalidHandle(999))
    );
    assert_eq!(calls.get(), 0);
    assert_eq!(
        real.call(Request::ReadStdin { stdin, max: 1 }),
        Ok(Completion::Input(Err(InputError::Canceled(1))))
    );
    assert_eq!(calls.get(), 0);
    for (max, expected) in [(2, vec![0, 255]), (10, vec![10]), (1, vec![])] {
        assert_eq!(
            real.call(Request::ReadStdin { stdin, max }),
            Ok(Completion::Input(Ok(expected)))
        );
    }
    for (name, expected) in [
        ("SET", Ok(Some("λ".into()))),
        ("UNSET", Ok(None)),
        ("SECRET", Err(EnvError::NotGranted)),
    ] {
        assert_eq!(
            real.call(Request::EnvGet {
                env: ENV_HANDLE,
                name: name.into()
            }),
            Ok(Completion::Env(expected))
        );
    }
    let Completion::Handle(Ok(stderr)) = real.call(Request::Stderr { io: 1 }).unwrap() else {
        panic!()
    };
    assert_eq!(
        real.call(Request::PrintStderr {
            stderr,
            bytes: b"separate\n".to_vec()
        }),
        Ok(Completion::Unit(Ok(())))
    );
    assert_eq!(real.stdout(), b"");
    assert_eq!(real.stderr(), b"separate\n");
    let tape = real.tape().to_vec();
    let encoded = hirsel_rt::launch::render_tape(&tape).unwrap();
    assert_eq!(hirsel_rt::launch::parse_tape(&encoded).unwrap(), tape);
    let mut replay = SimIo::new(Tree::new(BTreeMap::new()).unwrap(), tape.clone());
    replay.set_env(BTreeMap::from([
        ("SET".into(), Some("λ".into())),
        ("UNSET".into(), None),
    ]));
    replay.set_projections(Default::default());
    for (index, event) in tape.iter().enumerate() {
        if index == 1 {
            replay.set_projections(["stdin".into(), "stderr".into()].into_iter().collect());
        }
        match &event.completion {
            Completion::Refused(id) => assert_eq!(
                replay.call(event.request.clone()),
                Err(AdapterError::InvalidHandle(*id))
            ),
            _ => assert_eq!(
                replay.call(event.request.clone()),
                Ok(event.completion.clone())
            ),
        }
    }
    replay.finish().unwrap();
    assert_eq!(replay.stderr(), real.stderr());
    // Tape data alone cannot grant a name absent from admission.
    let event = tape
        .iter()
        .find(|e| matches!(&e.request,Request::EnvGet { name,.. } if name=="SET"))
        .unwrap()
        .clone();
    let mut denied = SimIo::new(Tree::new(BTreeMap::new()).unwrap(), vec![event.clone()]);
    assert_eq!(
        denied.call(event.request),
        Err(AdapterError::ReplayMismatch(0))
    );
    assert_eq!(denied.position(), 0);
}

// Rust fs::tests mkdir_path_already_exists_error, test_rename_directory_to_non_empty_directory,
// test_rename_file_over_open_file and test_rename_symlink, through confined provider requests.
#[test]
fn filesystem_lifecycle_replays_without_host_paths() {
    let (temp, mut host) = setup();
    std::os::unix::fs::symlink("missing", temp.path().join("link")).unwrap();
    std::os::unix::fs::symlink("../outside", temp.path().join("escape")).unwrap();
    let tree = host.snapshot().unwrap();
    let Completion::Handle(Ok(dir)) = host
        .call(Request::Directory {
            io: 1,
            path: "".into(),
        })
        .unwrap()
    else {
        panic!()
    };
    let mut expected = vec![host.tape().last().unwrap().clone()];
    {
        let mut call = |request: Request| {
            let result = host.call(request).unwrap();
            expected.push(host.tape().last().unwrap().clone());
            result
        };
        assert_eq!(
            call(Request::Mkdir {
                directory: dir,
                path: "new".into()
            }),
            Completion::Unit(Ok(()))
        );
        assert_eq!(
            call(Request::Mkdir {
                directory: dir,
                path: "new".into()
            }),
            Completion::Unit(Err(IoError::AlreadyExists))
        );
        assert_eq!(
            call(Request::Mkdir {
                directory: dir,
                path: "new/empty".into()
            }),
            Completion::Unit(Ok(()))
        );
        let Completion::Entries(Ok(entries)) = call(Request::ReadDir {
            directory: dir,
            path: "new".into(),
        }) else {
            panic!()
        };
        assert_eq!(
            entries,
            vec![DirEntry {
                path: "new/empty".into(),
                kind: FileKind::Directory
            }]
        );
        let Completion::Metadata(Ok(meta)) = call(Request::Metadata {
            directory: dir,
            path: "escape".into(),
        }) else {
            panic!()
        };
        assert_eq!(meta.kind, FileKind::Symlink);
        assert_eq!(meta.size, 10);
        assert!(meta.modified_unix_ns.is_some());
        assert_eq!(
            call(Request::Read {
                directory: dir,
                path: "escape".into()
            }),
            Completion::Bytes(Err(IoError::Fault(1)))
        );
        assert_eq!(
            call(Request::Rename {
                directory: dir,
                from: "link".into(),
                to: "renamed-link".into()
            }),
            Completion::Unit(Ok(()))
        );
        assert_eq!(
            call(Request::RemoveDir {
                directory: dir,
                path: "renamed-link".into()
            }),
            Completion::Unit(Err(IoError::NotDirectory))
        );
        assert_eq!(
            call(Request::Rename {
                directory: dir,
                from: "new".into(),
                to: "sub".into()
            }),
            Completion::Unit(Err(IoError::DirectoryNotEmpty))
        );
        assert_eq!(
            call(Request::RemoveDir {
                directory: dir,
                path: "new".into()
            }),
            Completion::Unit(Err(IoError::DirectoryNotEmpty))
        );
        assert_eq!(
            call(Request::Rename {
                directory: dir,
                from: "new".into(),
                to: "moved".into()
            }),
            Completion::Unit(Ok(()))
        );
        assert_eq!(
            call(Request::ReadDir {
                directory: dir,
                path: "moved".into()
            }),
            Completion::Entries(Ok(vec![DirEntry {
                path: "moved/empty".into(),
                kind: FileKind::Directory
            }]))
        );
        assert_eq!(
            call(Request::RemoveDir {
                directory: dir,
                path: "moved/empty".into()
            }),
            Completion::Unit(Ok(()))
        );
        assert_eq!(
            call(Request::RemoveDir {
                directory: dir,
                path: "moved".into()
            }),
            Completion::Unit(Ok(()))
        );
        assert_eq!(
            call(Request::RemoveDir {
                directory: dir,
                path: ".".into()
            }),
            Completion::Unit(Err(IoError::Fault(1)))
        );
        assert_eq!(
            call(Request::Rename {
                directory: dir,
                from: "old".into(),
                to: "".into()
            }),
            Completion::Unit(Err(IoError::Fault(1)))
        );
    }
    drop(host);
    drop(temp);
    let text = hirsel_rt::launch::render_tape(&expected).unwrap();
    let tape = hirsel_rt::launch::parse_tape(&text).unwrap();
    assert_eq!(tape, expected);
    let mut replay = SimIo::new(tree, tape.clone());
    for event in tape {
        assert_eq!(replay.call(event.request).unwrap(), event.completion);
    }
    replay.finish().unwrap();
}

#[test]
fn filesystem_mutations_deny_read_grants_before_resolving_paths_and_replay_cross_device() {
    use hirsel_run_schema::Access;
    let temp = Disposable::new();
    let pin = hirsel_rt::host_paths::PinnedDirectory::open(temp.path()).unwrap();
    let mut host =
        RealIo::from_pinned_mounts([("data".into(), Access::Read, pin)], std::io::sink()).unwrap();
    let Completion::Handle(Ok(dir)) = host
        .call(Request::Directory {
            io: 1,
            path: "data".into(),
        })
        .unwrap()
    else {
        panic!()
    };
    for request in [
        Request::Mkdir {
            directory: dir,
            path: "../outside".into(),
        },
        Request::RemoveDir {
            directory: dir,
            path: "missing/child".into(),
        },
        Request::Rename {
            directory: dir,
            from: "missing".into(),
            to: "/outside".into(),
        },
    ] {
        assert_eq!(
            host.call(request),
            Ok(Completion::Unit(Err(IoError::NotGranted)))
        );
        assert_eq!(host.tape().last().unwrap().fault, None);
    }
    assert!(std::fs::read_dir(temp.path()).unwrap().next().is_none());
    let tape = host.tape().to_vec();
    let mut replay = SimIo::new(Tree::new(Default::default()).unwrap(), tape.clone());
    replay.set_mounts([("data".into(), Access::Read)]).unwrap();
    for event in tape {
        assert_eq!(replay.call(event.request), Ok(event.completion));
    }
    replay.finish().unwrap();

    let (_temp, mut host) = setup();
    let Completion::Handle(Ok(dir)) = host
        .call(Request::Directory {
            io: 1,
            path: "".into(),
        })
        .unwrap()
    else {
        panic!()
    };
    host.inject(Fault {
        error: IoError::CrossDevice,
        partial: 0,
    });
    assert_eq!(
        host.call(Request::Rename {
            directory: dir,
            from: "old".into(),
            to: "new".into()
        }),
        Ok(Completion::Unit(Err(IoError::CrossDevice)))
    );
    let tape = hirsel_rt::launch::parse_tape(&hirsel_rt::launch::render_tape(host.tape()).unwrap())
        .unwrap();
    let mut replay = SimIo::new(initial(), tape.clone());
    for event in tape {
        assert_eq!(replay.call(event.request), Ok(event.completion));
    }
    replay.finish().unwrap();
    assert_eq!(replay.tree(), &initial());
}

#[test]
fn filesystem_metadata_abi_preserves_negative_nanoseconds_and_initializes_inactive_bytes() {
    struct MetadataOnly;
    impl Adapter for MetadataOnly {
        fn preflight(&self, request: &Request) -> Result<Option<Completion>, AdapterError> {
            assert!(matches!(request, Request::Metadata { directory: 7, .. }));
            Ok(None)
        }
        fn call(&mut self, request: Request) -> Result<Completion, AdapterError> {
            assert!(matches!(request, Request::Metadata { directory: 7, .. }));
            Ok(Completion::Metadata(Ok(Metadata {
                kind: FileKind::Symlink,
                size: 9,
                modified_unix_ns: Some(-1),
                permissions: 0o666,
            })))
        }
    }
    let ctx = support::Ctx::diagnostic();
    let mut binding = Box::new(Binding::new(MetadataOnly));
    let path = binding.intern_path("link");
    let mut out = MetadataResult {
        words: [u64::MAX; 10],
    };
    unsafe {
        binding.bind(&mut *ctx.0);
        assert_eq!(
            hirsel_rt_v2_file_metadata(ctx.0, std::ptr::null_mut(), &mut out, &7, &path),
            0
        );
    }
    assert_eq!(out.words, [0, 0, 2, 9, 1, 0, u64::MAX, u64::MAX, 0o666, 0]);
}

#[test]
fn filesystem_hard_link_rename_keeps_both_names_and_replays_inode_sharing() {
    let (temp, mut host) = setup();
    std::fs::hard_link(temp.path().join("old"), temp.path().join("other")).unwrap();
    let tree = host.snapshot().unwrap();
    assert_eq!(tree.file_aliases["other"], "old");
    let Completion::Handle(Ok(dir)) = host
        .call(Request::Directory {
            io: 1,
            path: "".into(),
        })
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(
        host.call(Request::Rename {
            directory: dir,
            from: "old".into(),
            to: "other".into()
        }),
        Ok(Completion::Unit(Ok(())))
    );
    assert!(temp.path().join("old").exists());
    let Completion::Handle(Ok(file)) = host
        .call(Request::Create {
            directory: dir,
            path: "other".into(),
        })
        .unwrap()
    else {
        panic!()
    };
    host.call(Request::FileAppend {
        directory: dir,
        file,
        bytes: b"shared".to_vec(),
    })
    .unwrap();
    host.call(Request::Close { handle: file }).unwrap();
    assert_eq!(
        host.call(Request::Read {
            directory: dir,
            path: "old".into()
        }),
        Ok(Completion::Bytes(Ok(b"shared".to_vec())))
    );
    host.call(Request::Rename {
        directory: dir,
        from: "old".into(),
        to: "new".into(),
    })
    .unwrap();
    host.call(Request::Remove {
        directory: dir,
        path: "other".into(),
    })
    .unwrap();
    let final_tree = host.snapshot().unwrap();
    let record = hirsel_rt::launch::Recording {
        arguments: vec![],
        run_identity: [0; 32],
        hash_seed: 0,
        random_seed: 0,
        test_seed: 0,
        binding: None,
        tree,
        events: host.tape().to_vec(),
    };
    let decoded =
        hirsel_rt::launch::parse_recording(&hirsel_rt::launch::render_recording(&record).unwrap())
            .unwrap();
    assert_eq!(decoded, record);
    drop(host);
    drop(temp);
    let mut replay = SimIo::new(decoded.tree, decoded.events.clone());
    for event in decoded.events {
        assert_eq!(replay.call(event.request), Ok(event.completion));
    }
    replay.finish().unwrap();
    assert_eq!(replay.tree(), &final_tree);
}

#[test]
fn filesystem_mount_pins_survive_rename_and_removal_before_directory_projection() {
    use hirsel_run_schema::Access;
    let temp = Disposable::new();
    std::fs::create_dir(temp.path().join("sub")).unwrap();
    let pin = |path: &Path| hirsel_rt::host_paths::PinnedDirectory::open(path).unwrap();
    let mut host = RealIo::from_pinned_mounts(
        [
            ("wide".into(), Access::ReadWrite, pin(temp.path())),
            (
                "narrow".into(),
                Access::ReadWrite,
                pin(&temp.path().join("sub")),
            ),
        ],
        std::io::sink(),
    )
    .unwrap();
    let tree = host.snapshot().unwrap();
    let Completion::Handle(Ok(wide)) = host
        .call(Request::Directory {
            io: 1,
            path: "wide".into(),
        })
        .unwrap()
    else {
        panic!()
    };
    host.call(Request::Rename {
        directory: wide,
        from: "sub".into(),
        to: "moved".into(),
    })
    .unwrap();
    let Completion::Handle(Ok(narrow)) = host
        .call(Request::Directory {
            io: 1,
            path: "narrow".into(),
        })
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(
        host.call(Request::Mkdir {
            directory: narrow,
            path: "child".into()
        }),
        Ok(Completion::Unit(Ok(())))
    );
    host.call(Request::RemoveDir {
        directory: narrow,
        path: "child".into(),
    })
    .unwrap();
    host.call(Request::RemoveDir {
        directory: wide,
        path: "moved".into(),
    })
    .unwrap();
    let Completion::Handle(Ok(removed)) = host
        .call(Request::Directory {
            io: 1,
            path: "narrow".into(),
        })
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(
        host.call(Request::ReadDir {
            directory: removed,
            path: "".into()
        }),
        Ok(Completion::Entries(Ok(vec![])))
    );
    assert_eq!(
        host.call(Request::Mkdir {
            directory: removed,
            path: "child".into()
        }),
        Ok(Completion::Unit(Err(IoError::NotFound)))
    );
    let final_tree = host.snapshot().unwrap();
    let tape = host.tape().to_vec();
    drop(host);
    drop(temp);
    let mut replay = SimIo::new(tree, tape.clone());
    replay
        .set_mounts([
            ("wide".into(), Access::ReadWrite),
            ("narrow".into(), Access::ReadWrite),
        ])
        .unwrap();
    for event in tape {
        assert_eq!(replay.call(event.request), Ok(event.completion));
    }
    replay.finish().unwrap();
    assert_eq!(replay.tree(), &final_tree);
}

#[test]
fn a_durable_journal_of_a_host_run_recovers_at_every_record_boundary_and_replays() {
    use hirsel_rt::journal::{DurableJournal, recover};
    use hirsel_rt::launch::Recording;
    let (temp, io) = setup();
    let path = temp.path().join("../").join(format!(
        "{}.journal",
        temp.path().file_name().unwrap().to_str().unwrap()
    ));
    let header = Recording {
        arguments: vec![],
        run_identity: [1; 32],
        hash_seed: 0,
        random_seed: 0,
        test_seed: 0,
        binding: None,
        tree: initial(),
        events: vec![],
    };
    let file = std::fs::File::create(&path).unwrap();
    let mut real = Library(io);
    real.0
        .journal_to(Box::new(DurableJournal::create(file, &header).unwrap()));
    actions(&mut real);
    let outcome = hirsel_run_schema::RunOutcome::Returned(hirsel_run_schema::Returned::Ok);
    real.0.finish_recording(&outcome).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    let tape = real.0.tape().to_vec();
    let recovered = recover(&bytes).unwrap();
    assert!(recovered.complete());
    assert_eq!(recovered.outcome, Some(outcome));
    assert_eq!(recovered.recording.events, tape);
    assert_eq!(recovered.recording.tree, initial());
    // Every request was prepared, in order, before its completion.
    assert_eq!(
        recovered
            .prepared
            .iter()
            .map(|p| &p.request)
            .collect::<Vec<_>>(),
        tape.iter().map(|e| &e.request).collect::<Vec<_>>()
    );
    // A crash at any byte leaves a valid prefix: its events begin the tape, and its requests
    // in doubt are exactly the prepared ones it does not complete.
    let mut boundaries = 0;
    for cut in 0..bytes.len() {
        let Ok(prefix) = recover(&bytes[..cut]) else {
            continue;
        };
        boundaries += usize::from(prefix.valid == cut);
        assert!(tape.starts_with(&prefix.recording.events));
        let completed = prefix.recording.events.len();
        let doubt: Vec<_> = prefix.prepared[completed..].to_vec();
        assert_eq!(prefix.in_doubt, doubt, "{cut}");
        assert_eq!(prefix.outcome, None);
    }
    assert!(boundaries > 2 * tape.len());
    // The complete journal replays exactly as its tape.
    let mut sim = SimIo::new(recovered.recording.tree.clone(), recovered.recording.events);
    for event in &tape {
        assert_eq!(
            sim.call(event.request.clone()),
            Ok(event.completion.clone())
        );
    }
    sim.finish().unwrap();
    assert_eq!(sim.stdout(), real.0.stdout());
    assert_eq!(sim.tree(), &real.0.snapshot().unwrap());
}

#[test]
fn retired_walk_text_recording_requires_recording_again() {
    let old = "hirsel-io-tape 7\nwalk 7 -> paths ok 61\n";
    let error = hirsel_rt::launch::parse_tape(old).unwrap_err();
    assert!(error.contains("retired Request::Walk"), "{error}");
    assert!(error.contains("re-record"), "{error}");
}
