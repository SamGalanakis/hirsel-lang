//! The backend-neutral emission interface: what every native backend receives
//! (`EmitInputs`), the names and codes it emits (link names, site codes,
//! descriptor names, failure codes), the receipt it must check before emitting
//! a unit, and the object manifest an emitted object must match.
//!
//! The LLVM and Cranelift backends both lower validated Ready units against
//! these definitions, so objects of either backend link with the same runtime
//! archive, launcher and observation layer.
use std::collections::{BTreeMap, BTreeSet};

use hirsel_core_contract::Manifest;
use hirsel_syntax::{ContentHash, DefPath};

use crate::abi::abi_digest;
use crate::abi_types::FnAbi;
use crate::ir::SiteRange;
use crate::layout_types::{
    LayoutId, LayoutKind, LayoutRecord, LayoutTable, ObjectFormat, ScalarKind, TargetSpec,
};
use crate::object::ObjectFile;
use crate::stage::{
    DebugSource, InputIdentity, ObjectArtifact, ObjectSymbol, ObjectSymbolKind, Ready, Relocation,
    Reproducibility, StageKind, ValidatedUnit,
};

/// How much a backend optimizes. LLVM runs `default<O0>`, `<O2>` and `<O3>`;
/// Cranelift has no optimizing profile beyond its own `speed` level.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OptLevel {
    /// No optimization.
    None,
    /// The ordinary executable build profile.
    Default,
    /// The release profile.
    Speed,
}

/// The runtime failure codes of one spec rule: `hirsel_rt_v2_failure_begin`'s `kind` and
/// `rule` (the failure category and the rule's index), assigned by the driver.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FailureCode {
    pub kind: u32,
    pub rule: u32,
}

/// Rule ID (`SEM-1`) to runtime failure codes. Every rule a failure edge starts must have one;
/// emission never invents a code.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FailureCodes {
    pub codes: BTreeMap<String, FailureCode>,
}

/// The rule a bounded arena's exhaustion fails with (MEM-6).
pub const BOUNDED_RULE: &str = "MEM-6";

/// The immutable inputs of one emission. The emitter receives no provider, query engine or
/// linker, and keeps no reference to any input after it returns.
#[derive(Clone, Debug)]
pub struct EmitInputs<'a> {
    /// The frozen front-end input every unit was built from.
    pub input: &'a InputIdentity,
    pub target: &'a TargetSpec,
    /// The table every unit's receipt names.
    pub layouts: &'a LayoutTable,
    /// The operation manifest every unit's receipt names.
    pub manifest: &'a Manifest,
    pub failures: &'a FailureCodes,
    /// The bytes of each static a `ReadStatic` names.
    pub statics: &'a [crate::statics::StaticData],
    /// Per layout with a user finalizer, the instance digest of its drop body's unit.
    pub finalizers: &'a BTreeMap<LayoutId, ContentHash>,
    /// Source specializations and known callable bodies replicated by the driver.
    pub inline_instances: &'a BTreeSet<ContentHash>,
    /// Small source units another object of the partition implements, emitted here as
    /// `available_externally` bodies so calls into them inline across objects.
    pub available_externally: &'a BTreeSet<ContentHash>,
    /// The definitions some unit of the program names as a C callback (PROG-4): each one's
    /// own object exports its `callback_thunk_name` thunk.
    pub callbacks: &'a BTreeSet<DefPath>,
    /// Each extern's `hirsel_rt_v2_foreign_enter` dispatch word (S-13,
    /// `runtime_abi::FOREIGN_THREADS_HOME` and the rest); an extern absent here is a
    /// non-blocking `home` call.
    pub foreign: &'a BTreeMap<DefPath, u64>,
    pub opt: OptLevel,
    pub registry: crate::descriptor_registry::DescriptorRegistry,
    /// Instrument inline count edges. Set only when linking a measurement runtime archive.
    pub measurement: bool,
    /// Charge the run's fuel at every fuel point (`crate::fuel::fuel_points`): the word at
    /// `runtime_abi::CTX_FUEL_OFFSET` loses one, and a negative result calls
    /// `hirsel_rt_v2_fuel_refill` and continues. Without it no code reads the word, so a run without a
    /// fuel budget pays nothing.
    pub fuel: bool,
}

/// C library symbols for byte comparisons and those a backend's lowering may introduce for
/// fused multiply-add and the exact rounding operations (`float.floor`, `ceil`, `trunc` and
/// `round`) on a target without an instruction for them, float remainder and memory
/// intrinsics, and the 128-bit division and integer-to-float conversion routines every C
/// toolchain's support library (libgcc, compiler-rt) provides.
pub const TOOLCHAIN_IMPORTS: [&str; 25] = [
    "__divti3",
    "__floattidf",
    "__floattisf",
    "__floatuntidf",
    "__floatuntisf",
    "__modti3",
    "__udivti3",
    "__umodti3",
    "bcmp",
    "ceil",
    "ceilf",
    "floor",
    "floorf",
    "fma",
    "fmaf",
    "fmod",
    "fmodf",
    "memcmp",
    "memcpy",
    "memmove",
    "memset",
    "round",
    "roundf",
    "trunc",
    "truncf",
];

/// The link name of a definition (and specialized instance). Injective and C-identifier safe:
/// `_H1`, then the package name, package version, modules and name, each as its escaped
/// length and escaped text, then `I` and the instance digest's first 16 hex digits when an
/// instance is named, then `E`. Escaping keeps `[A-Za-z0-9]`, doubles `_`, and writes any
/// other byte as `_` and two lowercase hex digits.
pub fn symbol_name(definition: &DefPath, instance: Option<&ContentHash>) -> String {
    fn escaped(out: &mut String, text: &str) {
        let mut piece = String::new();
        for byte in text.bytes() {
            match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' => piece.push(byte as char),
                b'_' => piece.push_str("__"),
                _ => piece.push_str(&format!("_{byte:02x}")),
            }
        }
        out.push_str(&piece.len().to_string());
        out.push_str(&piece);
    }
    let path = &definition.0;
    let mut name = String::from("_H1");
    escaped(&mut name, &path.package.name);
    escaped(&mut name, &path.package.version);
    for module in &path.modules {
        escaped(&mut name, module);
    }
    escaped(&mut name, &path.name);
    if let Some(instance) = instance {
        name.push('I');
        for byte in &instance.0[..8] {
            name.push_str(&format!("{byte:02x}"));
        }
    }
    name.push('E');
    name
}

/// The link name of a definition instance's function-reference thunk: the entry an indirect
/// call reaches through a callable made from the definition, taking (and ignoring) an
/// environment after the context. It is `symbol_name` with `T` after the `_H1` prefix; a
/// `symbol_name` segment always starts with a digit there, so no thunk name is a unit's name
/// and thunk names are injective as `symbol_name` is.
pub fn function_thunk_name(definition: &DefPath, instance: Option<&ContentHash>) -> String {
    let name = symbol_name(definition, instance);
    format!("_H1T{}", &name[3..])
}

/// The link name of a definition's C callback thunk (PROG-4): the C-convention entry
/// `R (*)(void *ctx, ...)` that calls the definition with the run context of its thread's
/// extern call. It is `symbol_name` with `C` after the `_H1` prefix, injective as
/// `function_thunk_name` is.
pub fn callback_thunk_name(definition: &DefPath, instance: Option<&ContentHash>) -> String {
    let name = symbol_name(definition, instance);
    format!("_H1C{}", &name[3..])
}

/// No callbacks, for inputs that name none.
pub static NO_CALLBACKS: BTreeSet<DefPath> = BTreeSet::new();
/// No extern dispatch words: every extern a non-blocking `home` call.
pub static NO_FOREIGN: BTreeMap<DefPath, u64> = BTreeMap::new();

/// A digest names the layout's runtime descriptor: one address identity across every object.
pub fn descriptor_name(layout: &LayoutRecord) -> String {
    let digest = crate::snapshot::digest("runtime-layout-descriptor", layout);
    let hex: String = digest.0.iter().map(|b| format!("{b:02x}")).collect();
    format!("hirsel.layout.{hex}")
}

/// The `site` code a failure passes to `hirsel_rt_v2_failure_begin`: the first eight bytes of
/// a digest of the definition, its implementation revision and the anchor's byte range. The
/// observation layer maps it back by recomputing it for the input's anchors.
pub fn site_code(definition: &DefPath, revision: &ContentHash, site: SiteRange) -> u64 {
    let digest = ContentHash::of_parts(
        "hirsel.native-site.v1",
        [
            definition.fingerprint().0.as_slice(),
            revision.0.as_slice(),
            &site.start.to_le_bytes(),
            &site.end.to_le_bytes(),
        ],
    );
    u64::from_le_bytes(digest.0[..8].try_into().expect("eight bytes")).max(1)
}

/// The counted sequence payload a slice of `element` owns: the table derives it with the
/// slice, whose owner word is that block's header address.
pub fn slice_storage(layouts: &LayoutTable, element: LayoutId) -> Option<LayoutId> {
    layouts
        .layouts
        .iter()
        .position(|l| l.kind == LayoutKind::Sequence { element })
        .map(|i| LayoutId(i as u32))
}

/// A word of the prefix an immortal block (static, region owner or arena block) carries
/// before its payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrefixWord {
    Capacity,
    Align,
    Count,
    Descriptor,
}

/// The immortal prefix's words in address order, ending at the payload: the extent
/// `{ capacity, align }` (`hirsel_rt_v2_extent`), then the counted header
/// `{ count, descriptor }`. Static blocks, region owners and arena blocks are laid out
/// from here and [`PrefixWord::bytes`].
pub const IMMORTAL_PREFIX: [PrefixWord; 4] = [
    PrefixWord::Capacity,
    PrefixWord::Align,
    PrefixWord::Count,
    PrefixWord::Descriptor,
];

/// Bytes of the immortal prefix; the payload lies at the first multiple of its physical
/// alignment at or past them.
pub const IMMORTAL_PREFIX_BYTES: u64 = PrefixWord::Capacity.back();

impl PrefixWord {
    pub const fn bytes(self) -> u64 {
        match self {
            PrefixWord::Capacity | PrefixWord::Align => 8,
            PrefixWord::Count | PrefixWord::Descriptor => 4,
        }
    }

    /// Bytes from this word's start to the end of the prefix.
    pub const fn back(self) -> u64 {
        let (mut i, mut found, mut bytes) = (0, false, 0);
        while i < IMMORTAL_PREFIX.len() {
            found |= IMMORTAL_PREFIX[i] as u8 == self as u8;
            if found {
                bytes += IMMORTAL_PREFIX[i].bytes();
            }
            i += 1;
        }
        bytes
    }
}

/// The scalar a value of this layout kind is held as in SSA form, if it is one.
pub fn scalar_shape(kind: &LayoutKind) -> Option<ScalarKind> {
    match kind {
        LayoutKind::Scalar(kind) => Some(*kind),
        LayoutKind::Path => Some(ScalarKind::U64),
        LayoutKind::Block { .. }
        | LayoutKind::Box { .. }
        | LayoutKind::Shared { .. }
        | LayoutKind::Header
        | LayoutKind::Arena => Some(ScalarKind::Pointer),
        _ => None,
    }
}

/// The scalar a Hirsel entry of `abi` returns in a register word, when its result is a
/// plain scalar of at most 64 bits. Every other result keeps its result pointer.
///
/// Register result invariant (ABI v2 lowering): such an entry returns the two-field
/// struct `{ i8 status, word }`, where the word is a pointer for a pointer result and 64 bits
/// otherwise. On success the word holds the value's bits in its low bits (Bool as one
/// byte, floats by bit pattern); emitted code zero-extends them and never assumes a
/// caller or callee did. On failure the word is zero and meaningless. Both targets
/// return the two fields in registers (RAX/AL and RDX, W0 and X1), which is also the C
/// struct `{ uint8_t status; uint64_t value; }` (or `void *value`). Definitions,
/// function-reference thunks, closure and iterator bodies follow it alike, so a direct,
/// a private and an indirect call agree; runtime primitives keep their result pointer.
pub fn register_result(layouts: &LayoutTable, abi: &FnAbi) -> Option<ScalarKind> {
    let result = abi.result.as_ref()?;
    let kind = scalar_shape(&layouts.layout(result.layout)?.kind)?;
    (!matches!(kind, ScalarKind::U128 | ScalarKind::I128)).then_some(kind)
}

/// Checks a unit's receipt against the inputs emission was given. A receipt binds the unit,
/// the layout table, the target, the manifest, the validator and the front-end input; any
/// other value means the unit was validated against something else. The error is the
/// problem, for the unit's definition.
pub fn check_receipt(unit: &ValidatedUnit<Ready>, inputs: &EmitInputs<'_>) -> Result<(), String> {
    let raw = unit.receipt().raw();
    let header = &unit.unit().header;
    let reject = |problem: &str| Err(problem.to_owned());
    if raw.stage != StageKind::Ready || header.stage != StageKind::Ready {
        return reject("the receipt is not for the Ready stage");
    }
    if raw.unit != unit.unit().digest() {
        return reject("the receipt names another unit");
    }
    if raw.lir_version != crate::LIR_VERSION || header.lir_version != crate::LIR_VERSION {
        return reject("the unit is of another LIR version");
    }
    if raw.validator_version != crate::VALIDATOR_VERSION {
        return reject("the receipt is from another validator version");
    }
    let target = inputs.target.digest();
    if raw.target != target || header.target != target || inputs.layouts.target != target {
        return reject("the receipt names another target");
    }
    let layouts = inputs.layouts.digest();
    if raw.layouts != layouts || header.layouts != layouts {
        return reject("the receipt names another layout table");
    }
    if raw.manifest != crate::snapshot::digest("manifest", inputs.manifest) {
        return reject("the receipt names another operation manifest");
    }
    if raw.input != *inputs.input || header.input != *inputs.input {
        return reject("the receipt names another front-end input");
    }
    if inputs.input.interface_version != crate::INTERFACE_VERSION {
        return reject("the input is of another interface version");
    }
    Ok(())
}

/// The symbols an emitter declared: what the object must export and may import.
#[derive(Default)]
pub struct Manifested {
    pub exports: BTreeMap<String, ObjectSymbol>,
    pub imports: BTreeMap<String, ObjectSymbol>,
    /// The `linkonce` library units (and replicated bodies) the object defines, which are no
    /// exports of it.
    pub library: BTreeSet<String>,
    /// Shared runtime descriptor and step globals emitted by this object.
    pub descriptors: BTreeSet<String>,
}

impl Manifested {
    /// Records an import of `name` unless it is already recorded.
    pub fn import(&mut self, name: &str, kind: ObjectSymbolKind, definition: Option<&DefPath>) {
        self.import_at(name, kind, definition, None);
    }

    /// `import` of a function at `abi`.
    pub fn import_at(
        &mut self,
        name: &str,
        kind: ObjectSymbolKind,
        definition: Option<&DefPath>,
        abi: Option<&FnAbi>,
    ) {
        self.imports
            .entry(name.to_owned())
            .or_insert_with(|| ObjectSymbol {
                name: name.to_owned(),
                kind,
                definition: definition.cloned(),
                abi: abi.map(abi_digest),
            });
    }

    /// An import the object also defines is internal to it.
    pub fn settle_imports(&mut self) {
        let (exports, library) = (&self.exports, &self.library);
        self.imports
            .retain(|name, _| !exports.contains_key(name) && !library.contains(name));
    }
}

/// Compares the object's symbol interface with the manifest and builds the artifact.
/// `toolchain` names the backend and its pinned version in the reproducibility record. The
/// error is the manifest problem.
pub fn artifact(
    file: ObjectFile,
    manifested: Manifested,
    units: &[&ValidatedUnit<Ready>],
    inputs: &EmitInputs<'_>,
    toolchain: &str,
) -> Result<ObjectArtifact, String> {
    if !file.thread_locals.is_empty() {
        return Err(format!(
            "the object uses thread-local storage, which a migrating task cannot: {}",
            file.thread_locals.join(", ")
        ));
    }
    let declared: Vec<&String> = manifested.exports.keys().collect();
    let exported: Vec<&String> = file
        .exports
        .iter()
        .filter(|name| {
            !manifested.library.contains(*name) && !manifested.descriptors.contains(*name)
        })
        .collect();
    if exported != declared {
        return Err(format!(
            "the object exports {:?}, the manifest {:?}",
            file.exports, declared
        ));
    }
    let mut imports = Vec::new();
    for name in &file.imports {
        let symbol = match manifested.imports.get(name) {
            Some(symbol) => symbol.clone(),
            None if TOOLCHAIN_IMPORTS.contains(&name.as_str()) => ObjectSymbol {
                name: name.clone(),
                kind: ObjectSymbolKind::Function,
                definition: None,
                abi: None,
            },
            None => {
                return Err(format!(
                    "the object imports `{name}`, which the manifest does not"
                ));
            }
        };
        imports.push(symbol);
    }
    let relocations = file
        .relocations
        .iter()
        .map(|r| Relocation {
            offset: r.offset,
            symbol: file
                .imports
                .iter()
                .position(|name| *name == r.symbol)
                .expect("a relocation's import is in the import list") as u32,
            kind: r.kind,
            addend: r.addend,
        })
        .collect();
    let exports: Vec<ObjectSymbol> = manifested.exports.into_values().collect();
    let abi = ContentHash::of_parts(
        "hirsel.object-abi.v1",
        exports
            .iter()
            .filter_map(|s| s.abi.as_ref())
            .map(|h| h.0.as_slice()),
    );
    let mut options = format!(
        "{:?} {} {} {}",
        inputs.opt,
        inputs.target.triple,
        inputs.target.cpu,
        inputs.target.features.join(",")
    );
    if inputs.measurement {
        options.push_str(" measurement");
    }
    if inputs.fuel {
        options.push_str(" fuel");
    }
    let mut debug: Vec<DebugSource> = units
        .iter()
        .map(|u| DebugSource {
            definition: u.unit().header.definition.clone(),
            implementation_revision: u.unit().header.implementation_revision,
        })
        .collect();
    debug.sort_by(|a, b| a.definition.cmp(&b.definition));
    debug.dedup();
    Ok(ObjectArtifact {
        input: inputs.input.clone(),
        target: inputs.target.digest(),
        format: ObjectFormat::Elf,
        bytes: file.bytes,
        exports,
        imports,
        relocations,
        debug,
        abi,
        layouts: inputs.layouts.digest(),
        reproducibility: Reproducibility {
            toolchain: toolchain.to_owned(),
            options: ContentHash::of_parts("hirsel.native-options.v1", [options.as_bytes()]),
            lir_version: crate::LIR_VERSION,
            runtime_abi_version: crate::RUNTIME_ABI_VERSION,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_syntax::{ItemPath, PackageId};

    fn definition(name: &str) -> DefPath {
        DefPath(ItemPath {
            package: PackageId {
                name: "fixture".into(),
                version: "0.1".into(),
            },
            modules: vec!["m_n".into()],
            name: name.into(),
        })
    }

    /// The immortal prefix is the runtime's extent and then its counted header, word by
    /// word (`hirsel_rt_v2.h`): the header's `HEADER_SIZE` bytes end at the prefix, with the
    /// count at `COUNT_OFFSET` and the descriptor at `LAYOUT_OFFSET`, and the extent's
    /// capacity, then its alignment, lie `IMMORTAL_CAPACITY_BACK` bytes before the header.
    /// A change to `IMMORTAL_PREFIX` or `PrefixWord::bytes` fails here, not in a
    /// capacity query that misreads a static block.
    #[test]
    fn the_immortal_prefix_is_the_runtime_extent_then_header() {
        use crate::runtime_abi::{
            COUNT_OFFSET, HEADER_SIZE, IMMORTAL_CAPACITY_BACK, LAYOUT_OFFSET,
        };
        let at = |word: PrefixWord| IMMORTAL_PREFIX_BYTES - word.back();
        let header = IMMORTAL_PREFIX_BYTES - HEADER_SIZE;
        assert_eq!(at(PrefixWord::Count), header + COUNT_OFFSET);
        assert_eq!(at(PrefixWord::Descriptor), header + LAYOUT_OFFSET);
        assert_eq!(
            PrefixWord::Count.bytes() + PrefixWord::Descriptor.bytes(),
            HEADER_SIZE
        );
        assert_eq!(header - at(PrefixWord::Capacity), IMMORTAL_CAPACITY_BACK);
        assert_eq!(
            at(PrefixWord::Align),
            at(PrefixWord::Capacity) + PrefixWord::Capacity.bytes()
        );
        assert_eq!(at(PrefixWord::Capacity), 0);
        assert_eq!(IMMORTAL_PREFIX_BYTES, 24);
    }

    #[test]
    fn link_names_are_injective_c_identifiers() {
        let a = symbol_name(&definition("f"), None);
        // `fixture`, `0.1` (`.` escaped), `m_n` (`_` doubled), `f`.
        assert_eq!(a, "_H17fixture50_2e14m__n1fE");
        let instance = ContentHash::of_bytes(b"i");
        let b = symbol_name(&definition("f"), Some(&instance));
        assert!(b.starts_with("_H17fixture") && b.contains('I') && b.ends_with('E'));
        assert_ne!(a, b);
        // `a_b` in one segment and `a`, `b` in two never collide.
        let mut split = definition("b");
        split.0.modules = vec!["a".into()];
        let mut joined = definition("a_b");
        joined.0.modules = Vec::new();
        assert_ne!(symbol_name(&split, None), symbol_name(&joined, None));
        // Function-reference thunks: a marker no unit name has, and injective as unit names
        // are, so no thunk collides with a unit or another thunk.
        let names = [
            (definition("f"), None),
            (definition("f"), Some(instance)),
            (split.clone(), None),
            (joined.clone(), None),
            (definition("T"), None),
        ];
        let mut all = BTreeSet::new();
        for (path, instance) in &names {
            let unit = symbol_name(path, instance.as_ref());
            let thunk = function_thunk_name(path, instance.as_ref());
            assert!(thunk.starts_with("_H1T") && !unit.starts_with("_H1T"));
            assert!(all.insert(unit) && all.insert(thunk));
        }
        assert_eq!(
            function_thunk_name(&definition("f"), None),
            "_H1T7fixture50_2e14m__n1fE"
        );
        let unicode = symbol_name(&definition("é"), None);
        assert!(
            unicode
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_'),
            "{unicode}"
        );
    }

    fn manifested(exports: &[&str], imports: &[&str]) -> Manifested {
        let symbol = |name: &&str| {
            (
                name.to_string(),
                ObjectSymbol {
                    name: name.to_string(),
                    kind: ObjectSymbolKind::Function,
                    definition: None,
                    abi: None,
                },
            )
        };
        Manifested {
            exports: exports.iter().map(symbol).collect(),
            imports: imports.iter().map(symbol).collect(),
            library: BTreeSet::new(),
            descriptors: BTreeSet::new(),
        }
    }

    fn file(exports: &[&str], imports: &[&str]) -> ObjectFile {
        ObjectFile {
            bytes: Vec::new(),
            exports: exports.iter().map(|s| s.to_string()).collect(),
            imports: imports.iter().map(|s| s.to_string()).collect(),
            relocations: Vec::new(),
            thread_locals: Vec::new(),
        }
    }

    fn check(file: ObjectFile, manifested: Manifested) -> Result<ObjectArtifact, String> {
        let target = TargetSpec::x86_64_unknown_linux_gnu();
        let layouts = LayoutTable::new(&target);
        let manifest = Manifest::skeleton([]);
        let failures = FailureCodes::default();
        let input = InputIdentity {
            interface_version: crate::INTERFACE_VERSION,
            digest: ContentHash::of_bytes(b"input"),
        };
        let finalizers = BTreeMap::new();
        let inputs = EmitInputs {
            input: &input,
            target: &target,
            layouts: &layouts,
            manifest: &manifest,
            failures: &failures,
            statics: &[],
            finalizers: &finalizers,
            inline_instances: &BTreeSet::new(),
            available_externally: &BTreeSet::new(),
            callbacks: &NO_CALLBACKS,
            foreign: &NO_FOREIGN,
            registry: crate::descriptor_registry::DescriptorRegistry::new(&layouts, &finalizers),
            opt: OptLevel::None,
            measurement: false,
            fuel: false,
        };
        artifact(file, manifested, &[], &inputs, "fixture")
    }

    #[test]
    fn the_object_interface_must_be_the_manifest() {
        check(
            file(
                &["f"],
                &[
                    "g",
                    "memcpy",
                    "__floattidf",
                    "__floattisf",
                    "__floatuntidf",
                    "__floatuntisf",
                ],
            ),
            manifested(&["f"], &["g", "h"]),
        )
        .expect("declared and toolchain imports are accepted; a declared import may be unused");
        for (file, manifested) in [
            (file(&["f", "x"], &[]), manifested(&["f"], &[])),
            (file(&[], &[]), manifested(&["f"], &[])),
            (file(&["f"], &["surprise"]), manifested(&["f"], &[])),
        ] {
            assert!(check(file, manifested).is_err());
        }
    }

    #[test]
    fn an_object_that_uses_thread_local_storage_is_refused() {
        let mut tls = file(&["f"], &[]);
        tls.thread_locals
            .push("TLS relocation type 23 in section 2".into());
        let refusal = check(tls, manifested(&["f"], &[])).unwrap_err();
        assert!(refusal.contains("thread-local storage"), "{refusal}");
    }
}
