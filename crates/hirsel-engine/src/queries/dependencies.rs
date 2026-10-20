//! Cached queries depend on observed lookup results and executed implementations.
use super::*;
use hirsel_resolve::{ManifestIndex, Namespace};
use hirsel_syntax::{PackageId, Path};
use hirsel_types::{DependencyRead, LookupError};

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct ExecutionReads {
    pub bodies: BTreeMap<DefPath, ContentHash>,
    pub signatures: BTreeMap<DefPath, ContentHash>,
    /// Each view reflection or a codec read, with the digest it saw.
    pub representations: BTreeMap<DefPath, ContentHash>,
}
#[derive(Clone, Default)]
pub(super) struct LookupInputs {
    namespaces: BTreeMap<DefPath, Option<Namespace>>,
}
impl LookupInputs {
    /// Every namespace input lies in packages `inside` accepts.
    pub(super) fn closed(&self, inside: &impl Fn(&PackageId) -> bool) -> bool {
        self.namespaces.keys().all(|p| inside(&p.0.package))
    }
}
impl ExecutionReads {
    pub(super) fn closed(&self, inside: &impl Fn(&PackageId) -> bool) -> bool {
        self.bodies
            .keys()
            .chain(self.signatures.keys())
            .chain(self.representations.keys())
            .all(|p| inside(&p.0.package))
    }
}
impl DependencyReads {
    /// Every read, positive or negative, names only packages `inside` accepts,
    /// so programs that select the same packages cannot change its outcome.
    pub(super) fn closed(&self, inside: &impl Fn(&PackageId) -> bool) -> bool {
        let read = |read: &DependencyRead| match read {
            DependencyRead::ProviderLookup {
                context,
                package,
                output,
                ..
            } => {
                inside(&context.package)
                    && package.as_ref().is_none_or(inside)
                    && output
                        .result
                        .as_ref()
                        .map_or(true, |p| inside(&p.0.package))
            }
            DependencyRead::Lookup {
                context,
                package,
                result,
                ..
            } => {
                inside(&context.package)
                    && package.as_ref().is_none_or(inside)
                    && result.as_ref().map_or(true, |p| inside(&p.0.package))
            }
            DependencyRead::ModuleHead { context, .. } => inside(&context.package),
            DependencyRead::Signature { definition, .. } => inside(&definition.0.package),
        };
        self.signatures
            .iter()
            .all(|s| inside(&s.definition.0.package))
            && self.failed_signatures.iter().all(|p| inside(&p.0.package))
            && self
                .failed_signature_keys
                .iter()
                .all(|(p, _)| inside(&p.0.package))
            && self.lookups.iter().all(|(p, _)| inside(&p.0.package))
            && self.manifest_lookups.iter().all(read)
            && self.resolver.iter().all(read)
    }
}
impl QueryEngine {
    fn manifest_index(&mut self, package: &PackageId) -> Option<Rc<ManifestIndex>> {
        Some(Rc::new(self.declarations.frozen().index(package.clone())))
    }
    fn provider_result(
        &mut self,
        context: &FileContext,
        package: &Option<PackageId>,
        segments: &[String],
    ) -> Result<DefPath, QueryError> {
        let path = Path {
            package: package.clone(),
            segments: segments.iter().map(|s| Symbol::intern(s)).collect(),
        };
        self.lookup(context, &path).result
    }
    fn lookup_result(
        &mut self,
        context: &FileContext,
        package: &Option<PackageId>,
        segments: &[String],
    ) -> Result<DefPath, LookupError> {
        let Some(index) = self.manifest_index(&context.package) else {
            return Err(LookupError::Missing);
        };
        let result = index.lookup_spelling(context, package.as_ref(), segments);
        if !matches!(
            result,
            Err(LookupError::Missing | LookupError::MissingMember(_))
        ) {
            return result;
        }
        match self.provider_result(context, package, segments) {
            Ok(path) => {
                if matches!(&result, Err(LookupError::MissingMember(candidates)) if !candidates.contains(&path))
                {
                    result
                } else {
                    index.access(context, &path)
                }
            }
            Err(QueryError::Diagnostics(ds)) if !ds.is_empty() => Err(LookupError::Provider(ds)),
            _ => result,
        }
    }
    pub(super) fn lookup_inputs(&mut self, reads: &DependencyReads) -> LookupInputs {
        let mut inputs = LookupInputs::default();
        for read in reads.resolver.iter().chain(&reads.manifest_lookups) {
            if let DependencyRead::Lookup {
                context, result, ..
            } = read
                && let Some(index) = self.manifest_index(&context.package)
            {
                if let Ok(path) = result {
                    inputs
                        .namespaces
                        .insert(path.clone(), index.entry(path).map(|e| e.namespace));
                }
            }
        }
        inputs
    }
    pub(super) fn lookup_reads_valid(
        &mut self,
        reads: &DependencyReads,
        inputs: &LookupInputs,
        owner: &DefPath,
    ) -> bool {
        for read in reads.resolver.iter().chain(&reads.manifest_lookups) {
            let unchanged = match read {
                DependencyRead::ProviderLookup {
                    context,
                    package,
                    segments,
                    output,
                } => self.provider_result(context, package, segments) == output.result,
                DependencyRead::Lookup {
                    context,
                    package,
                    segments,
                    result,
                    ..
                } => self.lookup_result(context, package, segments) == *result,
                DependencyRead::ModuleHead {
                    context,
                    head,
                    exposed,
                    ..
                } => self
                    .manifest_index(&context.package)
                    .is_some_and(|i| i.has_head(context, head) == *exposed),
                DependencyRead::Signature { .. } => true, // fingerprint validation is shared with provider reads
            };
            if !unchanged {
                return false;
            }
        }
        for (path, previous) in &inputs.namespaces {
            if self
                .manifest_index(&owner.0.package)
                .and_then(|i| i.entry(path).map(|e| e.namespace))
                != *previous
            {
                return false;
            }
        }
        true
    }
    pub(crate) fn implementation_hash(&self, path: &DefPath) -> Option<ContentHash> {
        let snapshot = self.items.get(path)?.first()?;
        Some(snapshot.text_hash)
    }
    pub(crate) fn executions_valid(&mut self, reads: &ExecutionReads) -> bool {
        reads
            .bodies
            .iter()
            .all(|(path, hash)| self.implementation_hash(path) == Some(*hash))
            && reads
                .signatures
                .iter()
                .all(|(path, hash)| self.signature(path).is_ok_and(|s| s.fingerprint == *hash))
            && reads
                .representations
                .iter()
                .all(|(path, digest)| self.representation_digest(path) == *digest)
    }
    pub(crate) fn inherit_execution(&mut self, reads: &ExecutionReads) {
        for frame in &mut self.execution_stack {
            frame.bodies.extend(reads.bodies.clone());
            frame.signatures.extend(reads.signatures.clone());
            frame.representations.extend(reads.representations.clone());
        }
    }
    /// Views compile-time reflection or a codec read during an evaluation.
    pub(crate) fn read_representations(&mut self, reads: &BTreeMap<DefPath, ContentHash>) {
        for frame in &mut self.execution_stack {
            frame.representations.extend(reads.clone());
        }
    }
    pub(crate) fn read_execution_signatures(&mut self, reads: &[SignatureDependency]) {
        for frame in &mut self.execution_stack {
            frame
                .signatures
                .extend(reads.iter().map(|d| (d.definition.clone(), d.fingerprint)));
        }
    }
    pub(crate) fn read_implementation(&mut self, path: &DefPath) {
        if let Some(hash) = self.implementation_hash(path) {
            for frame in &mut self.execution_stack {
                frame.bodies.insert(path.clone(), hash);
            }
        }
    }
}
