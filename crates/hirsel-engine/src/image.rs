//! The std image (design §1(f), docs/building.md "The std image"): a
//! dependency package checked once, by the toolchain build, into one file that
//! every program selecting the package maps instead of parsing and checking
//! the package.
//!
//! The image holds the package's declaration rows, every definition's public
//! signature with its private view and the inputs its readers observe, and,
//! per definition, the products running a program needs: its semantic node
//! and, for executable definitions, its lowered and validated TIR, completed
//! source audit and constants, with the execution reads computing them adds to
//! enclosing frames. Its type rows, numbered canonically, become the base of
//! every session interner that adopts it, so a decoded value is shared by all
//! of them. It is keyed by the compiler identity, the package's complete
//! checked input and the compile-time limits, and written only for a package
//! that checks complete with no diagnostics and whose queries read nothing
//! outside its closure. An image changes only how long a run takes.
use crate::snapshot::{DeclarationIndex, RowImage};
use crate::*;
use hirsel_image::{Encode, FIRST_KIND, Image};
use hirsel_span::Anchor;
use hirsel_syntax::{DefId, PackageId};
use hirsel_types::{Signature, SignatureRef, TypeBase, TypeDefinition};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::sync::{Arc, OnceLock};

/// The package and its sources (a blob).
pub(crate) const META: u32 = FIRST_KIND;
/// Each definition's declaration row.
pub(crate) const ROWS: u32 = FIRST_KIND + 1;
/// Each definition's public signature, coverage and private view.
pub(crate) const SIGNATURES: u32 = FIRST_KIND + 2;
/// The inputs each definition's readers observe.
pub(crate) const INPUTS: u32 = FIRST_KIND + 3;
/// Each definition's checked products.
pub(crate) const RECORDS: u32 = FIRST_KIND + 4;

/// The image's package and the definitions standing in each of its sources,
/// in manifest order.
#[derive(Serialize, Deserialize)]
pub(crate) struct Meta {
    pub package: PackageId,
    pub files: Vec<Vec<DefId>>,
    /// The generic generated declarations (CT-5), whose instances a program
    /// selects from their syntax.
    pub generated: Vec<DefId>,
}

/// A definition's signature as the image holds it.
#[derive(Serialize, Deserialize)]
pub(crate) struct SignatureEntry {
    pub signature: Signature,
    /// The query output's dependencies: what normalizing it read.
    pub dependencies: Vec<SignatureDependency>,
    /// The dependencies its fingerprint covers (`signature_coverage`).
    pub covered: Vec<SignatureDependency>,
    /// An opaque type's private shape and what normalizing it read.
    pub owner: Option<(TypeDefinition, Vec<SignatureDependency>)>,
}

/// A definition's decoded signature, shared by every session over the image.
pub(crate) struct ImageSignature {
    pub output: Arc<QueryOutput<SignatureRef>>,
    pub covered: Vec<SignatureDependency>,
    pub owner: Option<crate::representation::OwnerRecord>,
    /// The view the definition's own package's bodies see: the private view
    /// of an opaque type, else the public signature.
    pub private: SignatureRef,
}

/// The values a definition's readers observe that the image cannot recompute
/// without its syntax.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ImageInputs {
    /// Its first item's text hash (`ExecutedBody`, every body query's key).
    pub body: ContentHash,
    /// Its candidates' header hashes, combined (the signature query's key).
    pub header: ContentHash,
    /// `CitedText`.
    pub cited: ContentHash,
    /// Its first item's visibility and kind.
    pub names: ContentHash,
}

/// A definition's revision node.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct ImageNode {
    pub content: Vec<Vec<u8>>,
    pub dependencies: BTreeSet<DefPath>,
}

/// A definition's validated TIR, with what lowering it read.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ImageLowering {
    pub definition: hirsel_tir::DefinitionRef,
    pub dependencies: Vec<SignatureDependency>,
    pub execution: crate::memo::Reads,
    /// Present only after V accepted these exact immutable TIR bytes.
    pub verdict: Option<Vec<hirsel_validate::VRead>>,
}

/// One definition's checked products, decoded lazily and shared by every
/// session over the image.
#[derive(Debug, Serialize, Deserialize)]
pub struct ImageRecord {
    pub(crate) node: ImageNode,
    pub(crate) lowering: Option<ImageLowering>,
    pub(crate) audit: Option<crate::native_audit::SourceAuditProducts>,
    pub(crate) constant: Option<crate::comptime::EvaluatedConstant>,
}
impl ImageRecord {
    /// Whether the record holds the definition's TIR.
    pub(crate) fn has_lowering(&self) -> bool {
        self.lowering.is_some()
    }
    pub(crate) fn has_staged_reads(&self) -> bool {
        self.lowering
            .as_ref()
            .is_some_and(|lowering| crate::comptime::has_staged_reads(&lowering.definition))
    }
}
impl Encode for ImageRecord {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), String> {
        hirsel_image::encode_value(self, out)
    }
    /// TIR with a span outside the package's sources (a generated source) is
    /// left out; a reader computes it from source.
    fn reduced(&self) -> Option<Box<dyn Encode>> {
        self.lowering.as_ref()?;
        Some(Box::new(ImageRecord {
            node: ImageNode {
                content: self.node.content.clone(),
                dependencies: self.node.dependencies.clone(),
            },
            lowering: None,
            audit: self.audit.clone(),
            constant: self.constant.clone(),
        }))
    }
}
/// A value encoded whole or not at all.
pub(crate) struct Whole<T>(pub T);
impl<T: Serialize + Send> Encode for Whole<T> {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), String> {
        hirsel_image::encode_value(&self.0, out)
    }
}

/// A mapped std image.
pub struct StdImage {
    image: Image,
    package: PackageId,
    files: Vec<Vec<DefId>>,
    generated: BTreeSet<DefId>,
    /// Where each anchor stands: its source's manifest ordinal and its base.
    places: HashMap<Anchor, (u32, u32)>,
    /// The anchor at each place.
    at: HashMap<(u32, u32), Anchor>,
    declarations: DeclarationIndex,
    signatures: Box<[OnceLock<Option<Arc<ImageSignature>>>]>,
    inputs: Box<[OnceLock<Option<ImageInputs>>]>,
    records: Box<[OnceLock<Option<Arc<ImageRecord>>>]>,
}
impl std::fmt::Debug for StdImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "StdImage({}@{})",
            self.package.name, self.package.version
        )
    }
}

impl StdImage {
    /// The image key: the compiler identity, the package's checked-input
    /// revision (environment with the registry revision, closure manifests and
    /// source contents) and the compile-time limits its constants evaluated
    /// under.
    pub(crate) fn key_of(
        identity: ContentHash,
        revision: ContentHash,
        limits: hirsel_interp::Limits,
    ) -> ContentHash {
        ContentHash::of_parts(
            "hirsel.std-image.v1",
            [
                identity.0.as_slice(),
                revision.0.as_slice(),
                limits
                    .steps
                    .map(|steps| steps.to_le_bytes().to_vec())
                    .unwrap_or_default()
                    .as_slice(),
                (limits.call_depth as u64).to_le_bytes().as_slice(),
            ],
        )
    }
    /// Map the image at `p`. Its declaration rows and type rows decode now;
    /// everything else when first read.
    pub fn open(p: &Path) -> Result<Arc<Self>, String> {
        let image = Image::open(p).map_err(|e| format!("{}: {e}", p.display()))?;
        Self::from_image(image).map_err(|e| format!("{}: {e}", p.display()))
    }
    /// The image in `bytes`, as `open` reads a file.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Arc<Self>, String> {
        Self::from_image(Image::from_bytes(bytes)?)
    }
    fn from_image(image: Image) -> Result<Arc<Self>, String> {
        let meta: Meta = image.blob(META)?.ok_or("an image without its package")?;
        let mut declarations = DeclarationIndex::default();
        for ordinal in 0..image.entries(ROWS) {
            let row: RowImage = image.decode(ROWS, ordinal)?;
            let path = row.definition().clone();
            declarations.adopt_row(&path, row);
        }
        let places: HashMap<_, _> = image
            .anchors()
            .iter()
            .map(|&(a, ordinal, base)| (a, (ordinal, base)))
            .collect();
        let at = places.iter().map(|(&a, &place)| (place, a)).collect();
        Ok(Arc::new(Self {
            signatures: lazy(image.entries(SIGNATURES)),
            inputs: lazy(image.entries(INPUTS)),
            records: lazy(image.entries(RECORDS)),
            package: meta.package,
            files: meta.files,
            generated: meta.generated.into_iter().collect(),
            places,
            at,
            declarations,
            image,
        }))
    }
    pub fn key(&self) -> ContentHash {
        self.image.key()
    }
    /// The image's type rows: the core rows, then the package's, at the
    /// positions every value in it names.
    pub fn base(&self) -> Arc<TypeBase> {
        self.image.base()
    }
    pub fn package(&self) -> &PackageId {
        &self.package
    }
    /// The package's declaration rows.
    pub fn declarations(&self) -> &DeclarationIndex {
        &self.declarations
    }
    /// `d`'s public signature.
    pub fn signature(&self, d: DefId) -> Option<SignatureRef> {
        self.signature_entry(d)
            .map(|entry| entry.output.value.clone())
    }
    /// `d`'s checked products.
    pub fn record(&self, d: DefId) -> Option<Arc<ImageRecord>> {
        let ordinal = self.image.find(RECORDS, d)?;
        self.records[ordinal]
            .get_or_init(|| self.image.decode(RECORDS, ordinal).ok().map(Arc::new))
            .clone()
    }
    /// The definitions the image holds records for.
    pub fn definitions(&self) -> Vec<DefId> {
        self.image.keys(RECORDS).unwrap_or_default()
    }
    /// Whether the image declares `path`.
    pub(crate) fn declares(&self, path: &DefPath) -> bool {
        self.declarations.candidates(path).is_some()
    }
    pub(crate) fn signature_entry(&self, d: DefId) -> Option<Arc<ImageSignature>> {
        let ordinal = self.image.find(SIGNATURES, d)?;
        self.signatures[ordinal]
            .get_or_init(|| {
                let entry: SignatureEntry = self.image.decode(SIGNATURES, ordinal).ok()?;
                let public: SignatureRef = Arc::new(entry.signature);
                let body = self.inputs(d)?.body;
                let owner = entry.owner.map(|(declaration, dependencies)| {
                    crate::representation::OwnerRecord {
                        text_hash: body,
                        representation: hirsel_check_protocol::header::OwnerRepresentation {
                            declaration,
                        },
                        dependencies,
                    }
                });
                let private = match &owner {
                    Some(owner)
                        if public.metadata.visibility
                            == hirsel_syntax::Visibility::PublicOpaque =>
                    {
                        Arc::new(hirsel_types::encoding::owner_view(
                            &public,
                            owner.representation.declaration.clone(),
                        ))
                    }
                    _ => public.clone(),
                };
                Some(Arc::new(ImageSignature {
                    output: Arc::new(QueryOutput {
                        value: public,
                        diagnostics: vec![],
                        dependencies: entry.dependencies,
                    }),
                    covered: entry.covered,
                    owner,
                    private,
                }))
            })
            .clone()
    }
    pub(crate) fn inputs(&self, d: DefId) -> Option<ImageInputs> {
        let ordinal = self.image.find(INPUTS, d)?;
        *self.inputs[ordinal].get_or_init(|| self.image.decode(INPUTS, ordinal).ok())
    }
    /// The anchors spans in the image name, each with its source's manifest
    /// ordinal and its base there.
    pub(crate) fn anchors(&self) -> &[(Anchor, u32, u32)] {
        self.image.anchors()
    }
    /// Where `anchor` stands: its source's manifest ordinal and its base.
    pub(crate) fn place(&self, anchor: Anchor) -> Option<(u32, u32)> {
        self.places.get(&anchor).copied()
    }
    /// The anchor standing at `base` of the source at manifest `ordinal`.
    pub(crate) fn anchor_at(&self, ordinal: u32, base: u32) -> Option<Anchor> {
        self.at.get(&(ordinal, base)).copied()
    }
    /// The definitions standing in the source at manifest `ordinal`.
    pub(crate) fn file_definitions(&self, ordinal: u32) -> &[DefId] {
        self.files.get(ordinal as usize).map_or(&[], Vec::as_slice)
    }
    /// Whether `d` is a generic generated declaration (CT-5).
    pub(crate) fn generated(&self, d: DefId) -> bool {
        self.generated.contains(&d)
    }
    /// Whether the package has a generic generated declaration.
    pub(crate) fn generates(&self) -> bool {
        !self.generated.is_empty()
    }
    pub(crate) fn file_count(&self) -> usize {
        self.files.len()
    }
}

/// `count` values, each decoded when first read.
fn lazy<T>(count: usize) -> Box<[OnceLock<T>]> {
    (0..count).map(|_| OnceLock::new()).collect()
}

/// Which std images a program load adopts. A missing or stale image only
/// means checking the package from source.
#[derive(Clone, Default)]
pub struct StdImages(Option<Arc<ToolchainImages>>);
/// The toolchain's images: `$HIRSEL_IMAGE`, or `hirsel-std.image` beside the
/// executable.
pub(crate) struct ToolchainImages {
    pub images: Vec<Arc<StdImage>>,
    /// The host-selected file may be replaced when its checked inputs change.
    /// Explicit in-memory image sets never write to disk.
    pub path: Option<std::path::PathBuf>,
    /// Why there is no image, for `--verbose`.
    pub missing: Option<String>,
}
/// Write each later load's image notes (`LoadedProgram::image_notes`) to stderr.
static REPORT_IMAGES: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub fn report_images(on: bool) {
    REPORT_IMAGES.store(on, std::sync::atomic::Ordering::Relaxed);
}
pub(crate) fn reporting_images() -> bool {
    REPORT_IMAGES.load(std::sync::atomic::Ordering::Relaxed)
}
/// With a heap meter (`--mem-usage`), drop the process's retained signature
/// inputs and shared results, recording the bytes each held. Call it once
/// compiling is over.
pub fn release_shared_caches() {
    if !hirsel_intern::memory::metered() {
        return;
    }
    let shared = crate::store::MemStore::shared();
    hirsel_intern::memory::released("result store: mem@1", shared.take());
}
/// The toolchain image's path: `$HIRSEL_IMAGE`, or `hirsel-std.image` beside
/// the executable.
pub(crate) fn toolchain_image_path() -> Result<std::path::PathBuf, String> {
    match std::env::var_os("HIRSEL_IMAGE") {
        Some(path) => Ok(path.into()),
        None => std::env::current_exe()
            .map(|exe| exe.with_file_name("hirsel-std.image"))
            .map_err(|e| format!("no image: the executable's path is unknown ({e})")),
    }
}
fn toolchain_images() -> Arc<ToolchainImages> {
    static IMAGES: OnceLock<Arc<ToolchainImages>> = OnceLock::new();
    IMAGES
        .get_or_init(|| {
            let _timing =
                hirsel_intern::timings::interval(hirsel_intern::timings::Interval::ImageOpen);
            let path = toolchain_image_path();
            let opened = path.clone().and_then(|path| {
                hirsel_intern::memory::retained("std image", || StdImage::open(&path))
                    .map_err(|e| format!("no image: {e}"))
            });
            Arc::new(match opened {
                Ok(image) => ToolchainImages {
                    images: vec![image],
                    path: path.ok(),
                    missing: None,
                },
                Err(missing) => ToolchainImages {
                    images: vec![],
                    path: path.ok(),
                    missing: Some(missing),
                },
            })
        })
        .clone()
}
impl StdImages {
    /// No images: every package checks from source.
    pub fn none() -> Self {
        Self(None)
    }
    /// The toolchain's image (`$HIRSEL_IMAGE` or beside the executable).
    pub fn toolchain() -> Self {
        Self(Some(toolchain_images()))
    }
    /// The host-selected image file. A stale image is rebuilt atomically;
    /// a missing or unreadable file falls back to source checking.
    pub fn at(path: &Path) -> Self {
        let opened = StdImage::open(path);
        let (images, missing) = match opened {
            Ok(image) => (vec![image], None),
            Err(error) => (vec![], Some(format!("no image: {error}"))),
        };
        Self(Some(Arc::new(ToolchainImages {
            images,
            path: Some(path.to_path_buf()),
            missing,
        })))
    }
    /// Exactly `images`.
    pub fn of(images: Vec<Arc<StdImage>>) -> Self {
        Self(Some(Arc::new(ToolchainImages {
            images,
            path: None,
            missing: None,
        })))
    }
    pub(crate) fn get(&self) -> Option<&ToolchainImages> {
        self.0.as_deref()
    }
}

/// The definitions of each source, for `Meta`.
pub(crate) fn meta_files(definitions: &BTreeMap<u32, Vec<DefId>>, count: usize) -> Vec<Vec<DefId>> {
    (0..count as u32)
        .map(|ordinal| definitions.get(&ordinal).cloned().unwrap_or_default())
        .collect()
}
