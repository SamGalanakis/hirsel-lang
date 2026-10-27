use crate::*;
use hirsel_span::Span;
use hirsel_syntax::PackageId;

/// Every attempted manifest lookup is retained, including misses. The explicit
/// manifest revision closes the gap in SignatureDependency (spec issue #4).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum DependencyRead {
    ProviderLookup {
        context: ReadContext,
        package: Option<PackageId>,
        segments: ReadSegments,
        output: ReadLookupOutput,
    },
    Lookup {
        context: ReadContext,
        package: Option<PackageId>,
        segments: ReadSegments,
        manifest: ContentHash,
        result: Result<ReadPath, LookupError>,
    },
    ModuleHead {
        context: ReadContext,
        head: String,
        manifest: ContentHash,
        exposed: bool,
    },
    Signature {
        #[serde(with = "crate::definition_path")]
        definition: DefId,
        fingerprint: Option<ContentHash>,
    },
}

/// Contextual ordinals belong to this nominal owner, never to a file or Symbol.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum ContextualName {
    Constructor(DefPath),
    Variant {
        owner: DefPath,
        variant: VariantId,
    },
    Field {
        owner: DefPath,
        variant: Option<VariantId>,
        field: FieldId,
    },
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct DeferredName {
    pub site: NameSite,
    pub span: Span,
    pub name: String,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum GenericParameterName {
    Type(TypeParameterRef),
    Size(SizeParameterRef),
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct GenericBinding {
    pub owner: NameSite,
    pub name: Symbol,
    pub parameter: GenericParameterName,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub enum LookupError {
    Missing,
    /// A known module head was searched, retaining its candidates for diagnostics
    /// and cache invalidation when selected dependencies change.
    MissingMember(Vec<DefPath>),
    Ambiguous(Vec<DefPath>),
    Invisible(DefPath),
    /// Provider diagnostics are emitted only at a real use, never a reservation probe.
    Provider(Vec<Diagnostic>),
}
