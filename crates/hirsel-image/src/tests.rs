use crate::write::encode_value;
use crate::*;
use hirsel_intern::Symbol;
use hirsel_span::{Anchor, Span};
use hirsel_syntax::{ContentHash, DefId, DefPath, ItemPath, PackageId};
use hirsel_types::testing::Types;
use hirsel_types::*;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

const SIGNATURES: u32 = FIRST_KIND;
const RECORDS: u32 = FIRST_KIND + 1;
const INDEX: u32 = FIRST_KIND + 2;

fn path(modules: &[&str], name: &str) -> DefPath {
    DefPath(ItemPath {
        package: PackageId {
            name: "std".into(),
            version: "1".into(),
        },
        modules: modules.iter().map(|m| m.to_string()).collect(),
        name: name.into(),
    })
}

/// A value with every kind of id an image numbers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Value {
    types: Vec<TypeId>,
    names: Vec<Symbol>,
    defs: Vec<DefId>,
    span: Option<Span>,
    text: String,
}
impl Encode for Value {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), String> {
        encode_value(self, out)
    }
    /// Without its span, as a record leaves out TIR naming a foreign source.
    fn reduced(&self) -> Option<Box<dyn Encode>> {
        self.span.map(|_| -> Box<dyn Encode> {
            Box::new(Value {
                span: None,
                ..self.clone()
            })
        })
    }
}
/// A value that never encodes.
struct Broken;
impl Encode for Broken {
    fn encode(&self, _: &mut Vec<u8>) -> Result<(), String> {
        Err("broken".into())
    }
}

struct Fixture {
    types: Arc<TypeInterner>,
    /// Each definition's value, in DefPath order.
    values: Vec<(DefId, Value)>,
    index: Value,
    anchors: Vec<(Anchor, u32, u32)>,
}

/// Rows with symbols, definitions, callables and an invalid cyclic graph,
/// interned forwards or backwards, and values naming them.
fn fixture(backwards: bool) -> Fixture {
    let mut t = Types::default();
    let defs: Vec<DefId> = [
        path(&["text"], "Text"),
        path(&["list"], "push"),
        path(&[], "main"),
        path(&["io", "file"], "open"),
    ]
    .iter()
    .map(DefId::intern)
    .collect();
    let mut build: Vec<Box<dyn Fn(&mut Types) -> TypeId>> = vec![
        Box::new(|t| {
            let u8 = t.p(Primitive::U8);
            let list = t.list(u8);
            t.record(&[u8, list])
        }),
        Box::new(|t| {
            let text = t.named(&path(&["text"], "Text"), vec![]);
            let unit = t.unit();
            t.function(
                &[(hirsel_syntax::PassingMode::Borrow, text)],
                unit,
                Multiplicity::Many,
                1,
            )
        }),
        Box::new(|t| {
            let i64 = t.p(Primitive::I64);
            t.variant(&[&[i64], &[]])
        }),
        Box::new(|t| {
            // Two rows naming each other: an invalid graph the image keeps as written.
            let ids = t.store.intern_graph(2, |ids| {
                vec![
                    Type::with_kind(TypeKind::Box(ids[1])),
                    Type::with_kind(TypeKind::Tuple(vec![ids[0], TypeId::DANGLING].into())),
                ]
            });
            ids[0]
        }),
    ];
    if backwards {
        build.reverse();
    }
    let mut roots: Vec<TypeId> = build.iter().map(|f| f(&mut t)).collect();
    if backwards {
        roots.reverse();
    }
    let anchors: Vec<(Anchor, u32, u32)> =
        (0..3).map(|i| (Anchor::fresh(), i % 2, 10 * i)).collect();
    let mut values: Vec<(DefId, Value)> = defs
        .iter()
        .enumerate()
        .map(|(i, &d)| {
            (
                d,
                Value {
                    types: vec![roots[i], roots[(i + 1) % roots.len()]],
                    names: vec![
                        Symbol::intern(&format!("name{i}")),
                        Symbol::intern("shared"),
                    ],
                    defs: vec![d, defs[(i + 2) % defs.len()]],
                    span: Some(Span {
                        anchor: anchors[i % anchors.len()].0,
                        start: i as u32,
                        end: i as u32 + 3,
                    }),
                    text: format!("value {i}"),
                },
            )
        })
        .collect();
    values.sort_by(|a, b| a.0.cmp(&b.0));
    let index = Value {
        types: roots.clone(),
        names: vec![Symbol::intern("index")],
        defs: defs.clone(),
        span: None,
        text: "index".into(),
    };
    Fixture {
        types: t.store.clone(),
        values,
        index,
        anchors,
    }
}

fn key() -> ContentHash {
    ContentHash::of_bytes(b"image test key")
}

/// The fixture's image, its entries added in `order`.
fn write(f: &Fixture, backwards: bool) -> Written {
    let mut writer = ImageWriter::new(key(), f.types.clone());
    let mut anchors = f.anchors.clone();
    if backwards {
        anchors.reverse();
    }
    writer.anchors(anchors);
    let mut values = f.values.clone();
    if backwards {
        values.reverse();
    }
    for (d, value) in values {
        writer.entry(
            SIGNATURES,
            d,
            Box::new(Value {
                span: None,
                ..value.clone()
            }),
        );
        writer.entry(RECORDS, d, Box::new(value));
    }
    writer.blob(INDEX, Box::new(f.index.clone()));
    writer.finish().expect("an image")
}

/// A value's rows as a canonical table, comparable across interners.
fn canonical(t: &TypeInterner, value: &Value) -> TypeTable {
    encode_table(t, &value.types).0
}

#[test]
fn values_read_back_with_their_rows_names_definitions_and_spans() {
    let f = fixture(false);
    let written = write(&f, false);
    assert!(written.dropped.is_empty());
    let image = Image::from_bytes(written.bytes).unwrap();
    assert_eq!(image.key(), key());
    let session = TypeInterner::new(Some(image.base()));
    let anchors: Vec<Anchor> = image.anchors().iter().map(|a| a.0).collect();
    // Anchors are numbered by place: the fixture's three places, in order.
    let places: Vec<(u32, u32)> = image.anchors().iter().map(|a| (a.1, a.2)).collect();
    assert_eq!(places, [(0, 0), (0, 20), (1, 10)]);
    let keys = image.keys(RECORDS).unwrap();
    assert_eq!(keys, f.values.iter().map(|v| v.0).collect::<Vec<_>>());
    for (d, value) in &f.values {
        let read: Value = image.entry(RECORDS, *d).unwrap().expect("an entry");
        assert_eq!(read.names, value.names);
        assert_eq!(read.defs, value.defs);
        assert_eq!(read.text, value.text);
        let place = |anchor: Anchor, anchors: &[(Anchor, u32, u32)]| {
            anchors.iter().find(|a| a.0 == anchor).map(|a| (a.1, a.2))
        };
        let (written, read_span) = (value.span.unwrap(), read.span.unwrap());
        assert!(anchors.contains(&read_span.anchor));
        assert_eq!(
            place(read_span.anchor, image.anchors()),
            place(written.anchor, &f.anchors)
        );
        assert_eq!(
            (read_span.start, read_span.end),
            (written.start, written.end)
        );
        // Decoded ids are rows of the base, which every session shares.
        for ty in &read.types {
            assert!(session.contains(*ty));
        }
        assert_eq!(canonical(&session, &read), canonical(&f.types, value));
        let signature: Value = image.entry(SIGNATURES, *d).unwrap().unwrap();
        assert_eq!(signature.types, read.types);
    }
    let index: Value = image.blob(INDEX).unwrap().unwrap();
    assert_eq!(canonical(&session, &index), canonical(&f.types, &f.index));
    assert_eq!(index.defs, f.index.defs);
    // Interning a row the base holds finds the base's id.
    let first = session.row(index.types[0]).clone();
    assert_eq!(session.intern(first), index.types[0]);
    assert_eq!(session.stats().rows, 0, "no row was added beside the base");
}

#[test]
fn absent_entries_and_definitions_read_as_none() {
    let f = fixture(false);
    let image = Image::from_bytes(write(&f, false).bytes).unwrap();
    let elsewhere = DefId::intern(&path(&["absent"], "nothing"));
    assert_eq!(image.entry::<Value>(RECORDS, elsewhere).unwrap(), None);
    assert_eq!(
        image.entry::<Value>(FIRST_KIND + 9, f.values[0].0).unwrap(),
        None
    );
    assert_eq!(image.blob::<Value>(FIRST_KIND + 9).unwrap(), None);
    assert_eq!(image.entries(FIRST_KIND + 9), 0);
}

#[test]
fn bytes_do_not_depend_on_interning_or_entry_order() {
    let forwards = write(&fixture(false), false).bytes;
    let backwards = write(&fixture(true), true).bytes;
    assert!(forwards == backwards, "reordering changed the image");
}

#[test]
fn bytes_do_not_depend_on_id_assignment() {
    let image = || {
        let bytes = write(&fixture(false), false).bytes;
        format!("{} {:?}", bytes.len(), ContentHash::of_bytes(&bytes).0)
    };
    let Some(runs) = hirsel_intern::under_salts(
        "tests::bytes_do_not_depend_on_id_assignment",
        &[1, 2, 0x5eed],
        image,
    ) else {
        return;
    };
    assert!(runs.iter().all(|run| *run == runs[0]), "{runs:#?}");
    assert_eq!(runs[0], image(), "salted runs agree with the unsalted one");
}

#[test]
fn an_entry_that_does_not_encode_is_reduced_or_dropped() {
    let f = fixture(false);
    let mut writer = ImageWriter::new(key(), f.types.clone());
    // No anchors: every span names a source outside the image.
    let (d, value) = f.values[0].clone();
    writer.entry(RECORDS, d, Box::new(value.clone()));
    writer.entry(RECORDS, f.values[1].0, Box::new(Broken));
    let written = writer.finish().unwrap();
    assert_eq!(written.dropped.len(), 1);
    assert_eq!(
        (written.dropped[0].0, written.dropped[0].1),
        (RECORDS, f.values[1].0)
    );
    let image = Image::from_bytes(written.bytes).unwrap();
    let read: Value = image.entry(RECORDS, d).unwrap().unwrap();
    assert_eq!(read.span, None, "the reduced value stands in");
    assert_eq!(read.names, value.names);
    assert_eq!(image.entry::<Value>(RECORDS, f.values[1].0).unwrap(), None);
}

#[test]
fn damaged_images_are_refused_not_trusted() {
    let bytes = write(&fixture(false), false).bytes;
    assert!(
        Image::from_bytes(b"not an image at all, but long enough to have a header".to_vec())
            .is_err()
    );
    let mut version = bytes.clone();
    version[8] ^= 1;
    assert!(Image::from_bytes(version).is_err());
    for cut in (0..HEADER + 3).chain([bytes.len() / 2, bytes.len() - 1]) {
        assert!(
            Image::from_bytes(bytes[..cut].to_vec()).is_err(),
            "cut at {cut}"
        );
    }
    // A section whose range overflows is refused, not read past the file.
    let mut overflow = bytes.clone();
    overflow[HEADER + 8..HEADER + 16].copy_from_slice(&u64::MAX.to_le_bytes());
    assert!(Image::from_bytes(overflow).is_err());
}

#[test]
fn an_image_file_is_mapped_and_replaced_whole() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("hirsel-std.image");
    let f = fixture(false);
    let bytes = write(&f, false).bytes;
    write_atomically(&file, &bytes).unwrap();
    assert_eq!(Image::read_key(&file).unwrap(), key());
    let image = Image::open(&file).unwrap();
    assert_eq!(image.len(), bytes.len());
    // Replacing the file leaves an open image reading the one it mapped.
    write_atomically(&file, b"HIRSIMG1 but nothing else").unwrap();
    let read: Value = image.entry(RECORDS, f.values[0].0).unwrap().unwrap();
    assert_eq!(read.text, f.values[0].1.text);
    assert!(Image::open(&file).is_err());
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        1,
        "no temporary file is left"
    );
}

#[test]
fn concurrent_image_replacements_publish_only_complete_files() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("hirsel-std.image");
    let start = std::sync::Barrier::new(8);
    std::thread::scope(|scope| {
        for byte in 0..8u8 {
            let (file, start) = (&file, &start);
            scope.spawn(move || {
                let bytes = vec![byte; 64 * 1024];
                start.wait();
                write_atomically(file, &bytes).unwrap();
            });
        }
    });
    let bytes = std::fs::read(&file).unwrap();
    assert_eq!(bytes.len(), 64 * 1024);
    assert!(bytes.iter().all(|byte| *byte == bytes[0]));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn an_image_over_a_based_interner_keeps_the_base_rows_and_their_ids() {
    let f = fixture(false);
    let core = encode_table(&f.types, &f.index.types).0.into_base();
    let types = TypeInterner::new(Some(core.clone()));
    let shared = TypeBase::from_table(&encode_table(&f.types, &f.index.types).0).1;
    let u8 = types.intern(Type::with_kind(TypeKind::Primitive(Primitive::U8)));
    let added = types.intern(Type::with_kind(TypeKind::Tuple(vec![shared[0], u8].into())));
    let d = f.values[0].0;
    let value = Value {
        types: vec![added, shared[1]],
        names: vec![],
        defs: vec![],
        span: None,
        text: String::new(),
    };
    let mut writer = ImageWriter::new(key(), types.clone());
    writer.entry(RECORDS, d, Box::new(value.clone()));
    let image = Image::from_bytes(writer.finish().unwrap().bytes).unwrap();
    assert!(image.base().len() > core.len());
    let read: Value = image.entry(RECORDS, d).unwrap().unwrap();
    // A base row keeps its id; a row beyond the base is a new base row.
    assert_eq!(read.types[1], shared[1]);
    let session = TypeInterner::new(Some(image.base()));
    assert_eq!(canonical(&session, &read), canonical(&types, &value));
}

#[test]
fn shared_anchor_decoding_refuses_invalid_numbers_and_keeps_later_reads_exact() {
    #[derive(Clone)]
    struct RawSpan(u32);
    impl Encode for RawSpan {
        fn encode(&self, out: &mut Vec<u8>) -> Result<(), String> {
            encode_value(&(self.0, 7u32, 9u32), out)
        }
    }
    let t = TypeInterner::new(None);
    let anchor = Anchor::fresh();
    let mut writer = ImageWriter::new(key(), t);
    writer.anchors([(anchor, 2, 31)]);
    let ids: Vec<_> = ["absent", "present", "invalid"]
        .map(|n| DefId::intern(&path(&[], n)))
        .into();
    for (&id, number) in ids.iter().zip([0, 1, 99]) {
        writer.entry(RECORDS, id, Box::new(RawSpan(number)));
    }
    let image = Image::from_bytes(writer.finish().unwrap().bytes).unwrap();
    let mapped = image.anchors()[0].0;
    for _ in 0..2 {
        assert!(image.entry::<Span>(RECORDS, ids[2]).is_err());
        assert_eq!(
            image.entry::<Span>(RECORDS, ids[1]).unwrap().unwrap(),
            Span {
                anchor: mapped,
                start: 7,
                end: 9
            }
        );
        assert_eq!(
            image
                .entry::<Span>(RECORDS, ids[0])
                .unwrap()
                .unwrap()
                .anchor,
            Span::default().anchor
        );
    }
}

#[test]
fn assembly_reuses_a_section_buffer_and_preserves_directory_order() {
    // The records section has room for the image already. Final assembly
    // must not request another image-sized allocation beside it.
    for largest in 0..3 {
        let mut sections = vec![
            (16, 1, vec![1; 3]),
            (17, 2, vec![2; 5]),
            (18, 3, vec![3; 7]),
        ];
        sections[largest].2.reserve(1024);
        let allocation = sections[largest].2.as_ptr();
        let expected: Vec<u8> = sections.iter().flat_map(|s| s.2.iter().copied()).collect();
        let metadata: Vec<_> = sections
            .iter()
            .map(|(kind, count, body)| (*kind, *count, body.len()))
            .collect();
        let bytes = crate::write::assemble(key(), sections).unwrap();
        assert_eq!(
            bytes.as_ptr(),
            allocation,
            "assembly allocated a second image"
        );
        let directory = HEADER + metadata.len() * DIRECTORY_ENTRY;
        assert_eq!(&bytes[directory..], expected);
        let mut at = directory;
        for (i, (kind, count, len)) in metadata.iter().enumerate() {
            let entry = HEADER + i * DIRECTORY_ENTRY;
            assert_eq!(&bytes[entry..entry + 4], kind.to_le_bytes());
            assert_eq!(&bytes[entry + 4..entry + 8], count.to_le_bytes());
            assert_eq!(&bytes[entry + 8..entry + 16], (at as u64).to_le_bytes());
            assert_eq!(&bytes[entry + 16..entry + 24], (*len as u64).to_le_bytes());
            at += len;
        }
    }
}

#[test]
fn keyed_directory_compacts_omissions_before_between_and_after_values() {
    let mut writer = ImageWriter::new(key(), TypeInterner::new(None));
    let defs: Vec<_> = (0..5)
        .map(|i| DefId::intern(&path(&[], &format!("entry{i}"))))
        .collect();
    for (i, &d) in defs.iter().enumerate() {
        let value: Box<dyn Encode> = if i % 2 == 0 {
            Box::new(Broken)
        } else {
            Box::new(Value {
                types: vec![],
                names: vec![],
                defs: vec![],
                span: None,
                text: format!("value{i}"),
            })
        };
        writer.entry(RECORDS, d, value);
    }
    let written = writer.finish().unwrap();
    assert_eq!(written.dropped.len(), 3);
    let image = Image::from_bytes(written.bytes).unwrap();
    assert_eq!(image.keys(RECORDS).unwrap(), [defs[1], defs[3]]);
    for i in [1, 3] {
        assert_eq!(
            image
                .entry::<Value>(RECORDS, defs[i])
                .unwrap()
                .unwrap()
                .text,
            format!("value{i}")
        );
    }
    let mut writer = ImageWriter::new(key(), TypeInterner::new(None));
    writer.entry(RECORDS, defs[0], Box::new(Broken));
    let image = Image::from_bytes(writer.finish().unwrap().bytes).unwrap();
    assert!(image.keys(RECORDS).unwrap().is_empty());
}
