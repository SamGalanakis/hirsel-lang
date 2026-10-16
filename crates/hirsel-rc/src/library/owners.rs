//! The library's owner representations: the private shapes of `Map`, `Set` and `SortedMap`, the borrowed
//! cursor, the three consuming-cursor records with their finalizer paths,
//! the two string views and `StrPos`.
//!
//! These are declarations. Each is a `RepresentationRecord` over ordinary
//! types, so its layout, scan, drop and copy programs are derived by B and
//! never written by hand. What other code may rely on: field 0 of
//! a container is `entries`; a `Cursor` is three words; a consuming cursor
//! is `{ source, next: u64 }`.
use super::{REVISION, core_path, native_path};
use hirsel_lir::layout::{Container, OwnerRepresentation, Owners};
use hirsel_syntax::{ContentHash, DefPath};
use hirsel_tir::{BodyId, RepresentationRecord};
use hirsel_types::{
    BinderRef, BuiltinType, DropAllocation, Field, GenericArgument, GenericParameter, Layout,
    Modes, Primitive, SignatureKind, SignatureRef, Type, TypeId, TypeKind, TypeParamId, TypeStore,
};
use std::collections::BTreeMap;

/// What the library owns of a program's `Owners`.
#[derive(Clone, Debug, Default)]
pub struct LibraryOwners {
    pub nominals: BTreeMap<DefPath, OwnerRepresentation>,
    pub containers: BTreeMap<Container, RepresentationRecord>,
}

impl LibraryOwners {
    /// The program's owners with the library's laid over them: a library
    /// type's native representation is the library's, whatever the registry
    /// declaration says about its public shape.
    pub fn over(self, program: &Owners) -> Owners {
        let mut out = program.clone();
        out.nominals.extend(self.nominals);
        out.containers.extend(self.containers);
        out
    }
}

/// `core@1/$intrinsic.Cursor`, the borrowed loop cursor.
pub fn cursor() -> DefPath {
    core_path("$intrinsic.Cursor")
}

/// The consuming cursors: the nominal, the container or sequence it
/// consumes, and its finalizer body's `$native` name.
pub const CONSUMING: [(&str, Source, &str); 3] = [
    ("$intrinsic.OwnedCursor", Source::Array, "owned_cursor_drop"),
    (
        "$intrinsic.OwnedMapCursor",
        Source::Map,
        "owned_map_cursor_drop",
    ),
    (
        "$intrinsic.OwnedSortedMapCursor",
        Source::SortedMap,
        "owned_sorted_map_cursor_drop",
    ),
];

/// What a consuming cursor consumes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Source {
    /// `Array[T]`: `ListConsume` and `ArrayConsume` move the block in
    /// unchanged, `InlineConsume` allocates one.
    Array,
    Map,
    SortedMap,
}

/// The string views, each with the layout of `String`.
pub const VIEWS: [&str; 2] = ["ScalarView", "PositionView"];

/// Field 0 of every container shape and of every consuming cursor.
pub const ENTRIES: u32 = 0;
/// `Map` and `Set`: the open-addressing index.
pub const INDEX: u32 = 1;
/// A consuming cursor's position.
pub const NEXT: u32 = 1;

/// A hand-built shape: a store whose type parameters are the owner's own,
/// by ordinal.
#[derive(Default)]
struct Shape {
    store: TypeStore,
}

impl Shape {
    fn add(&mut self, kind: TypeKind) -> TypeId {
        self.store.push(Type {
            sendability_requirement: hirsel_types::SendabilityRequirement::None,
            allocation_target: None,
            drop_allocation: DropAllocation::NoAlloc,
            kind,
            modes: Modes::global(),
            erased_captures: None,
            brand: Default::default(),
        })
    }
    fn primitive(&mut self, p: Primitive) -> TypeId {
        self.add(TypeKind::Primitive(p))
    }
    fn parameter(&mut self, ordinal: u32) -> TypeId {
        self.add(TypeKind::Parameter(BinderRef::bound(TypeParamId(ordinal))))
    }
    fn builtin(&mut self, kind: BuiltinType, arguments: &[TypeId]) -> TypeId {
        self.add(TypeKind::Builtin {
            kind,
            arguments: arguments
                .iter()
                .map(|&ty| GenericArgument::Type(ty))
                .collect::<Vec<_>>()
                .into(),
        })
    }
    fn record(&mut self, fields: &[(&str, TypeId)]) -> TypeId {
        self.add(TypeKind::Record(
            fields
                .iter()
                .map(|&(name, ty)| Field {
                    growable: false,
                    name: name.into(),
                    ty,
                })
                .collect(),
        ))
    }
    /// `List[(K, V)]` over parameters 0 and 1.
    fn pairs(&mut self) -> TypeId {
        let k = self.parameter(0);
        let v = self.parameter(1);
        let pair = self.add(TypeKind::Tuple(vec![k, v].into()));
        self.builtin(BuiltinType::List, &[pair])
    }
    fn index(&mut self) -> TypeId {
        let slot = self.primitive(Primitive::U32);
        self.builtin(BuiltinType::List, &[slot])
    }
}

/// The representation revision of a library shape: the library revision,
/// the shape's name and the declaration it represents.
fn revision(name: &str, signature: Option<&SignatureRef>) -> ContentHash {
    let declared = signature.map(|s| s.fingerprint.0).unwrap_or_default();
    ContentHash::of_parts(
        "hirsel.library-owner.v1",
        [
            REVISION.to_le_bytes().as_slice(),
            name.as_bytes(),
            declared.as_slice(),
        ],
    )
}

fn type_parameters(names: &[&str]) -> Vec<GenericParameter> {
    names
        .iter()
        .map(|name| GenericParameter::Type {
            name: (*name).into(),
            bounds: Vec::new().into(),
        })
        .collect()
}

/// A nominal's generics as its declaration states them, so the record's
/// arity is the declaration's.
fn declared(signature: Option<&SignatureRef>, otherwise: &[&str]) -> Vec<GenericParameter> {
    match signature.map(|s| &s.kind) {
        Some(SignatureKind::Type(t)) => t.generics.clone(),
        _ => type_parameters(otherwise),
    }
}

fn record(
    definition: DefPath,
    name: &str,
    signature: Option<&SignatureRef>,
    generics: Vec<GenericParameter>,
    shape: Shape,
    private_shape: TypeId,
) -> RepresentationRecord {
    RepresentationRecord {
        definition,
        representation_revision: revision(name, signature),
        generics,
        types: shape.store,
        private_shape,
        layout_promises: Layout::default(),
        resource: false,
        finalizer: None,
        unsafe_ffi_package: false,
    }
}

/// The library's owner representations: the four containers, the cursor
/// types and the string views (§2.5, §2.6). `signatures` are the program's
/// registry declarations; a type the program does not declare gets no row.
pub fn owners(signatures: &BTreeMap<DefPath, SignatureRef>) -> LibraryOwners {
    let mut out = LibraryOwners::default();

    for (name, resource) in [
        ("Path", false),
        ("File", true),
        ("AtomicFile", true),
        ("Stream", true),
        ("Listener", true),
    ] {
        if let Some(signature) = signatures.get(&core_path(name)) {
            out.nominals.insert(
                core_path(name),
                OwnerRepresentation::Handle {
                    resource,
                    revision: signature.fingerprint,
                },
            );
        }
    }

    // Containers. Field 0, `entries`, is dense and the public contract;
    // `index` is lib-map's and empty means unindexed.
    let mut s = Shape::default();
    let entries = s.pairs();
    let index = s.index();
    let shape = s.record(&[("entries", entries), ("index", index)]);
    out.containers.insert(
        Container::Map,
        record(
            core_path("Map"),
            "Map",
            None,
            type_parameters(&["K", "V"]),
            s,
            shape,
        ),
    );
    let mut s = Shape::default();
    let element = s.parameter(0);
    let entries = s.builtin(BuiltinType::List, &[element]);
    let index = s.index();
    let shape = s.record(&[("entries", entries), ("index", index)]);
    out.containers.insert(
        Container::Set,
        record(
            core_path("Set"),
            "Set",
            None,
            type_parameters(&["T"]),
            s,
            shape,
        ),
    );
    let mut s = Shape::default();
    let entries = s.pairs();
    let shape = s.record(&[("entries", entries)]);
    out.containers.insert(
        Container::SortedMap,
        record(
            core_path("SortedMap"),
            "SortedMap",
            None,
            type_parameters(&["K", "V"]),
            s,
            shape,
        ),
    );

    // The borrowed cursor: three words for every `T`.
    let path = cursor();
    if let Some(signature) = signatures.get(&path) {
        out.nominals.insert(
            path,
            OwnerRepresentation::Cursor {
                revision: revision("Cursor", Some(signature)),
            },
        );
    }

    let path = core_path("$intrinsic.ViewCursor");
    if let Some(signature) = signatures.get(&path) {
        out.nominals.insert(
            path,
            OwnerRepresentation::ViewCursor {
                revision: revision("ViewCursor", Some(signature)),
            },
        );
    }

    // The consuming cursors: a resource over its source and a position,
    // whose finalizer is a library body.
    for (name, source, finalizer) in CONSUMING {
        let path = core_path(name);
        let Some(signature) = signatures.get(&path) else {
            continue;
        };
        let mut s = Shape::default();
        let held = match source {
            Source::Array => {
                let element = s.parameter(0);
                s.builtin(BuiltinType::Array, &[element])
            }
            Source::Map | Source::SortedMap => {
                let k = s.parameter(0);
                let v = s.parameter(1);
                let kind = if source == Source::Map {
                    BuiltinType::Map
                } else {
                    BuiltinType::SortedMap
                };
                s.builtin(kind, &[k, v])
            }
        };
        let next = s.primitive(Primitive::U64);
        let shape = if matches!(source, Source::Array | Source::Map | Source::SortedMap) {
            let unique = s.primitive(Primitive::Bool);
            s.record(&[("source", held), ("next", next), ("unique", unique)])
        } else {
            s.record(&[("source", held), ("next", next)])
        };
        let generics = declared(
            Some(signature),
            if source == Source::Array {
                &["T"]
            } else {
                &["K", "V"]
            },
        );
        let mut r = record(path.clone(), name, Some(signature), generics, s, shape);
        r.resource = true;
        r.finalizer = Some((native_path(finalizer), BodyId(0)));
        out.nominals.insert(path, OwnerRepresentation::Defined(r));
    }

    // The string views have the layout of `String`; `StrPos` is a byte
    // offset from the slice's start (its brand is static).
    for name in VIEWS {
        out.nominals
            .insert(core_path(name), OwnerRepresentation::StringView);
    }
    let path = core_path("StrPos");
    if let Some(signature) = signatures.get(&path) {
        let mut s = Shape::default();
        let private_shape = s.primitive(Primitive::U64);
        out.nominals.insert(
            path.clone(),
            OwnerRepresentation::Defined(record(
                path,
                "StrPos",
                Some(signature),
                declared(Some(signature), &[]),
                s,
                private_shape,
            )),
        );
    }
    out
}
