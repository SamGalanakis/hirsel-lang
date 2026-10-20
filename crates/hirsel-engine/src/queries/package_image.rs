//! Dependency packages read, parsed, adopted and validated once per process.
//! Fresh engines install an image's immutable maps as the base of their own
//! overlays; nothing an engine does reaches the image (README, package images).
use super::*;
use crate::package_cache::{Artifact, PackageCache, SharedLowerings};
use hirsel_syntax::PackageId;
use std::path::PathBuf;

type Base<K, V> = Arc<BTreeMap<K, V>>;

/// One selected dependency package and the publications an image adopts for it.
#[derive(Clone)]
pub(crate) struct ImagePackage {
    pub package: PackageId,
    pub closure: BTreeSet<PackageId>,
    pub artifact: Option<Arc<Artifact>>,
    pub lowerings: Option<SharedLowerings>,
}

/// What an image is built from. Program files are placements only: their text
/// and paths never enter an image or its identity.
pub(crate) struct ImageInput {
    /// Selected `unsafe ffi` packages; their manifests are in `manifests`.
    pub unsafe_ffi: BTreeMap<PackageId, hirsel_types::UnsafeFfi>,
    pub environment: QueryEnvironment,
    pub limits: hirsel_interp::Limits,
    pub manifests: BTreeMap<PackageId, ContentHash>,
    pub exposures: BTreeMap<PackageId, Vec<hirsel_resolve::DependencyExposure>>,
    /// Every selected file in FileId order; `None` text marks a program file.
    pub files: Vec<(PathBuf, FileContext, Option<String>)>,
    pub packages: Vec<ImagePackage>,
}

#[derive(Clone)]
struct Adopted {
    package: ImagePackage,
    lowerings: BTreeSet<DefPath>,
    proven: bool,
}

pub(crate) struct PackageImage {
    /// Sources appended while adopting generated bodies, in FileId order.
    sources: Vec<(FileId, hirsel_span::SourceFile)>,
    generated: BTreeMap<DefPath, GeneratedSource>,
    source_revision: u64,
    cache_sequence: u64,
    items: Base<DefPath, Vec<ItemSnapshot>>,
    parsed: Base<FileId, ParsedFile>,
    syntax_keys: Base<(DefPath, QueryKind), DefinitionKey>,
    signatures: Base<DefPath, Arc<Cached<hirsel_types::SignatureRef>>>,
    resolutions: Base<DefPath, Arc<Cached<Resolution>>>,
    checked: Base<DefPath, Arc<Cached<TypedBody>>>,
    modes: Base<DefPath, Arc<Cached<AnalyzedBody>>>,
    lowered: Base<DefPath, Arc<Cached<hirsel_tir::DefinitionRef>>>,
    reads: Base<(DefPath, QueryKind), DependencyReads>,
    resolver_outputs: Base<DefPath, crate::ResolverSnapshot>,
    signature_resolutions: Base<DefPath, crate::ResolverSnapshot>,
    representations: Base<DefPath, crate::representation::OwnerRecord>,
    constant_values: Base<DefPath, crate::comptime::EvaluatedConstant>,
    mode_diagnostics: Vec<(Diagnostic, String)>,
    adopted: Vec<Adopted>,
    /// The parse-only image publications were adopted on.
    parsed_only: Option<Arc<PackageImage>>,
}

impl ImageInput {
    pub(crate) fn key(&self) -> ContentHash {
        let environment = &self.environment;
        let mut parts = vec![
            environment.interface_version.to_le_bytes().to_vec(),
            environment.compiler_version.as_bytes().to_vec(),
            environment.spec_hash.0.to_vec(),
            environment.builtin_signatures_hash.0.to_vec(),
            self.limits
                .steps
                .map(|steps| steps.to_le_bytes().to_vec())
                .unwrap_or_default(),
            (self.limits.call_depth as u64).to_le_bytes().to_vec(),
        ];
        for (package, manifest) in &self.manifests {
            parts.extend([
                package.name.as_bytes().to_vec(),
                package.version.as_bytes().to_vec(),
                manifest.0.to_vec(),
            ]);
        }
        for (path, context, text) in &self.files {
            let Some(text) = text else {
                parts.push(b"program".to_vec());
                continue;
            };
            parts.extend([
                path.as_os_str().as_encoded_bytes().to_vec(),
                context.package.name.as_bytes().to_vec(),
                context.package.version.as_bytes().to_vec(),
                context.modules.join("/").into_bytes(),
                ContentHash::of_bytes(text.as_bytes()).0.to_vec(),
            ]);
        }
        ContentHash::of_parts("hirsel.package-image.v1", parts.iter().map(Vec::as_slice))
    }
    /// The published lowerings not already in each package's artifact.
    fn lowerings(&self) -> Vec<BTreeMap<DefPath, Arc<crate::package_cache::PublishedLowering>>> {
        self.packages
            .iter()
            .map(|p| {
                p.lowerings.as_ref().map_or_else(BTreeMap::new, |l| {
                    l.lock()
                        .unwrap()
                        .iter()
                        .filter(|(path, _)| {
                            !p.artifact
                                .as_ref()
                                .is_some_and(|a| a.snapshot.contains_lowering(path))
                        })
                        .map(|(path, l)| (path.clone(), l.clone()))
                        .collect()
                })
            })
            .collect()
    }
}

fn same<T>(a: &Option<Arc<T>>, b: &Option<Arc<T>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        _ => false,
    }
}

impl PackageImage {
    /// The image for `input`, built at most once per identity and publication
    /// set in `cache` (always built afresh for a disabled cache).
    pub(crate) fn obtain(
        cache: &PackageCache,
        input: &ImageInput,
        adapters: impl Fn() -> PhaseAdapters,
    ) -> Arc<PackageImage> {
        let _timing = timings::interval(Interval::S0);
        let key = input.key();
        let lowerings = input.lowerings();
        let current = cache.image(key);
        let unchanged = current.as_ref().filter(|image| {
            image.adopted.len() == input.packages.len()
                && image.adopted.iter().zip(&input.packages).all(|(a, p)| {
                    same(&a.package.artifact, &p.artifact)
                        && same(&a.package.lowerings, &p.lowerings)
                })
        });
        let image = match unchanged {
            Some(image)
                if image
                    .adopted
                    .iter()
                    .zip(&lowerings)
                    .all(|(a, l)| l.keys().all(|p| a.lowerings.contains(p))) =>
            {
                return image.clone();
            }
            // Only new lowerings were published: extend this image with them.
            Some(image) => {
                let new: Vec<_> = image
                    .adopted
                    .iter()
                    .zip(&lowerings)
                    .map(|(a, l)| {
                        l.iter()
                            .filter(|(p, _)| !a.lowerings.contains(*p))
                            .map(|(p, l)| (p.clone(), l.clone()))
                            .collect()
                    })
                    .collect();
                let parsed_only = image.parsed_only.clone();
                Arc::new(Self::adopt(
                    image,
                    input,
                    None,
                    new,
                    adapters(),
                    parsed_only,
                ))
            }
            None => {
                let parsed = match current {
                    Some(image) => image.parsed_only.clone().unwrap_or(image),
                    None => Arc::new(Self::parse(input, adapters())),
                };
                if input
                    .packages
                    .iter()
                    .zip(&lowerings)
                    .all(|(p, l)| p.artifact.is_none() && l.is_empty())
                {
                    parsed
                } else {
                    let artifacts = input.packages.iter().map(|p| p.artifact.clone()).collect();
                    Arc::new(Self::adopt(
                        &parsed,
                        input,
                        Some(artifacts),
                        lowerings,
                        adapters(),
                        Some(parsed.clone()),
                    ))
                }
            }
        };
        cache.store_image(key, image.clone());
        image
    }

    fn builder(input: &ImageInput, adapters: PhaseAdapters) -> QueryEngine {
        let mut engine = QueryEngine::new(input.environment.clone(), adapters);
        engine.comptime_limits = input.limits;
        engine.package_manifests = input.manifests.clone();
        {
            let mut state = engine
                .adapters
                .compiler_state
                .as_ref()
                .expect("package images need compiler state")
                .borrow_mut();
            state.package_exposures = input.exposures.clone();
            state.signature_metadata.unsafe_ffi = input.unsafe_ffi.clone();
        }
        for (path, _, text) in &input.files {
            engine
                .sources
                .add(path, text.clone().unwrap_or_default())
                .expect("selected sources fit the source map");
        }
        engine
    }

    /// Parse every dependency file in FileId order. A file that fails keeps its
    /// diagnostics; loading stops there, so later files stay unparsed.
    fn parse(input: &ImageInput, adapters: PhaseAdapters) -> PackageImage {
        let mut engine = Self::builder(input, adapters);
        for (index, (_, context, text)) in input.files.iter().enumerate() {
            if text.is_some() && engine.parse_file(FileId(index as u32), context).is_err() {
                break;
            }
        }
        let adopted = input
            .packages
            .iter()
            .map(|p| Adopted {
                package: ImagePackage {
                    artifact: None,
                    lowerings: None,
                    ..p.clone()
                },
                lowerings: BTreeSet::new(),
                proven: false,
            })
            .collect();
        Self::freeze(engine, adopted, None, input.files.len())
    }

    /// Adopt `artifacts` (when given) and `lowerings` on `base`, rebasing their
    /// FileIds to this placement once, then validate every package once.
    fn adopt(
        base: &PackageImage,
        input: &ImageInput,
        artifacts: Option<Vec<Option<Arc<Artifact>>>>,
        lowerings: Vec<BTreeMap<DefPath, Arc<crate::package_cache::PublishedLowering>>>,
        adapters: PhaseAdapters,
        parsed_only: Option<Arc<PackageImage>>,
    ) -> PackageImage {
        let mut engine = Self::builder(input, adapters);
        engine.install_image(base);
        let mut ordinals = BTreeMap::<&PackageId, u32>::new();
        let sites: BTreeMap<_, _> = input
            .files
            .iter()
            .enumerate()
            .map(|(file, (_, context, _))| {
                let ordinal = ordinals.entry(&context.package).or_default();
                *ordinal += 1;
                (
                    (context.package.clone(), *ordinal as usize - 1),
                    FileId(file as u32),
                )
            })
            .collect();
        let moves = |sites_of: &[(FileId, PackageId, usize, u32)]| -> Vec<_> {
            sites_of
                .iter()
                .map(|(old, id, index, len)| (*old, sites[&(id.clone(), *index)], *len))
                .collect()
        };
        if let Some(artifacts) = &artifacts {
            for artifact in artifacts.iter().flatten() {
                engine.adopt_package_snapshot(&artifact.snapshot, &moves(&artifact.sites));
            }
        }
        for lowering in lowerings.iter().flat_map(BTreeMap::values) {
            engine.adopt_package_snapshot(&lowering.snapshot, &moves(&lowering.sites));
        }
        let adopted = base
            .adopted
            .iter()
            .zip(&input.packages)
            .zip(lowerings)
            .map(|((old, package), new)| {
                let mut adopted = old.lowerings.clone();
                adopted.extend(new.into_keys());
                Adopted {
                    package: package.clone(),
                    lowerings: adopted,
                    proven: false,
                }
            })
            .collect::<Vec<_>>();
        let adopted = adopted
            .into_iter()
            .map(|mut a| {
                a.proven = a.package.artifact.is_some()
                    && engine.package_snapshot_valid(&a.package.package)
                    && engine.package_closed(&a.package.package, &a.package.closure);
                a
            })
            .collect();
        Self::freeze(engine, adopted, parsed_only, input.files.len())
    }

    fn freeze(
        mut engine: QueryEngine,
        adopted: Vec<Adopted>,
        parsed_only: Option<Arc<PackageImage>>,
        source_count: usize,
    ) -> PackageImage {
        let mut sources = vec![];
        while let Some(file) = u32::try_from(source_count + sources.len()).ok().map(FileId)
            && let Some(source) = engine.sources.file(file)
        {
            sources.push((file, source.clone()));
        }
        let state = engine.adapters.compiler_state.take().unwrap();
        let mut state = state.borrow_mut();
        PackageImage {
            sources,
            generated: std::mem::take(&mut engine.generated),
            source_revision: engine.source_revision,
            cache_sequence: engine.cache_sequence,
            items: engine.items.freeze(),
            parsed: engine.parsed.freeze(),
            syntax_keys: engine.syntax_keys.freeze(),
            signatures: engine.signatures.freeze(),
            resolutions: engine.resolutions.freeze(),
            checked: engine.checked.freeze(),
            modes: engine.modes.freeze(),
            lowered: engine.lowered.freeze(),
            reads: engine.reads.freeze(),
            resolver_outputs: state.resolver_outputs.freeze(),
            signature_resolutions: state.signature_resolutions.freeze(),
            representations: state.representations.freeze(),
            constant_values: state.constant_values.freeze(),
            mode_diagnostics: std::mem::take(&mut state.mode_diagnostics),
            adopted,
            parsed_only,
        }
    }

    /// Dependency packages whose validated reads are closed under their
    /// dependency closure: their proof holds in every program selecting them.
    pub(crate) fn proven(&self, package: &PackageId) -> bool {
        self.adopted
            .iter()
            .any(|a| a.proven && &a.package.package == package)
    }
}

impl QueryEngine {
    /// Start this engine's caches and source revision from `image`.
    /// The engine's sources must hold the image's files at the same FileIds.
    pub(crate) fn install_image(&mut self, image: &PackageImage) {
        let _timing = timings::interval(Interval::S0);
        for (file, source) in &image.sources {
            let placed = self
                .sources
                .add(&source.path, source.text().into())
                .expect("cached generated sources fit the source map");
            assert_eq!(placed, *file, "package image source placement");
        }
        self.generated = image.generated.clone();
        self.source_revision = image.source_revision;
        self.cache_sequence = image.cache_sequence;
        self.items = Layered::shared(image.items.clone());
        self.parsed = Layered::shared(image.parsed.clone());
        self.syntax_keys = Layered::shared(image.syntax_keys.clone());
        self.signatures = Layered::shared(image.signatures.clone());
        self.resolutions = Layered::shared(image.resolutions.clone());
        self.checked = Layered::shared(image.checked.clone());
        self.modes = Layered::shared(image.modes.clone());
        self.lowered = Layered::shared(image.lowered.clone());
        self.reads = Layered::shared(image.reads.clone());
        let mut state = self
            .adapters
            .compiler_state
            .as_ref()
            .expect("package images need compiler state")
            .borrow_mut();
        state.resolver_outputs = Layered::shared(image.resolver_outputs.clone());
        state.signature_resolutions = Layered::shared(image.signature_resolutions.clone());
        state.representations = Layered::shared(image.representations.clone());
        state.constant_values = Layered::shared(image.constant_values.clone());
        state.mode_diagnostics = image.mode_diagnostics.clone();
        state.materialization_environment = Some(self.environment.clone());
        state.materialization_limits = Some(self.comptime_limits);
        drop(state);
        self.publish_declarations();
    }

    /// No retained query of `package` reads outside its closure (or core).
    pub(super) fn package_closed(
        &self,
        package: &PackageId,
        closure: &BTreeSet<PackageId>,
    ) -> bool {
        let inside = inside(closure);
        macro_rules! closed {
            ($field:ident) => {
                self.$field
                    .values()
                    .filter(|c| &c.key.definition.0.package == package)
                    .all(|c| c.closed(&inside))
            };
        }
        let state = self.adapters.compiler_state.as_ref().unwrap().borrow();
        closed!(signatures)
            && closed!(resolutions)
            && closed!(checked)
            && closed!(modes)
            && closed!(lowered)
            && self
                .reads
                .iter()
                .filter(|((p, _), _)| &p.0.package == package)
                .all(|(_, reads)| reads.closed(&inside))
            && state
                .constant_values
                .iter()
                .filter(|(p, _)| &p.0.package == package)
                .all(|(_, value)| value.executed.closed(&inside))
    }
    /// Release the checked, mode and lowered bodies of `paths` and the TIR V
    /// remembers passing for them; a later read computes them again. Whether
    /// each released query read only inside `closure` (or core), as
    /// `package_closed` would have found it.
    pub(super) fn release_bodies(
        &mut self,
        paths: &[DefPath],
        closure: &BTreeSet<PackageId>,
    ) -> bool {
        let inside = inside(closure);
        let mut closed = true;
        for path in paths {
            closed &= self.checked.get(path).is_none_or(|c| c.closed(&inside))
                && self.modes.get(path).is_none_or(|c| c.closed(&inside))
                && self.lowered.get(path).is_none_or(|c| c.closed(&inside));
            self.checked.remove(path);
            self.modes.remove(path);
            self.lowered.remove(path);
        }
        self.forget_passed(paths);
        closed
    }
}

/// Membership in a package's closure, where core is always inside.
fn inside(closure: &BTreeSet<PackageId>) -> impl Fn(&PackageId) -> bool {
    let core = hirsel_syntax::DefPath::core("Result").0.package;
    move |p: &PackageId| closure.contains(p) || *p == core
}

impl<T> Cached<T> {
    /// The query read nothing outside `inside`.
    fn closed(&self, inside: &impl Fn(&PackageId) -> bool) -> bool {
        self.reads.closed(inside)
            && self.lookups.closed(inside)
            && self.executed.closed(inside)
            && self.representations.keys().all(|p| inside(&p.0.package))
    }
}
