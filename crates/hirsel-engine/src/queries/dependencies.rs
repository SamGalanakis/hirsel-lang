//! What queries observe: lookups through the manifest index and the
//! provider, and the implementations an evaluation executes.
use super::*;
use crate::memo::ReadKey;
use hirsel_resolve::ManifestIndex;
use hirsel_syntax::{PackageId, Path};
use hirsel_types::LookupError;

impl QueryEngine {
    pub(super) fn manifest_index(&mut self, package: &PackageId) -> Option<Arc<ManifestIndex>> {
        Some(Arc::new(self.declarations.frozen().index(package.clone())))
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
    pub(super) fn lookup_result(
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
    pub(crate) fn implementation_hash(&self, path: &DefPath) -> Option<ContentHash> {
        match self.items.get(path) {
            Some(items) => Some(items.first()?.text_hash),
            None => self
                .catalog
                .body(path)
                .or_else(|| Some(self.image_inputs(path)?.body)),
        }
    }
    /// Every open evaluation frame read these views by reflecting or
    /// serializing them, each with the digest it saw.
    pub(crate) fn read_representations(&mut self, reads: &BTreeMap<DefPath, ContentHash>) {
        for frame in &mut self.execution_stack {
            for (path, digest) in reads {
                frame.record(ReadKey::Representation(DefId::intern(path)), *digest);
            }
        }
    }
    /// Every open evaluation frame read these signatures by executing.
    pub(crate) fn read_execution_signatures(&mut self, reads: &[SignatureDependency]) {
        if self.execution_stack.is_empty() {
            return;
        }
        // A body's dependency names the view it was checked against, public or
        // its package's private one; the read observes the public signature,
        // as `observe` does, so every frame sees one value per definition.
        // An evaluation's own private artifact names no definition.
        let named: Vec<_> = reads
            .iter()
            .filter(|d| self.names_definition(d.definition.path()))
            .collect();
        let observed: Vec<_> = named
            .into_iter()
            .map(|d| {
                let fingerprint = self.signature(d.definition).ok().map(|s| s.fingerprint);
                (
                    ReadKey::ExecutedSignature(d.definition),
                    memos::signature_value(fingerprint),
                )
            })
            .collect();
        for frame in &mut self.execution_stack {
            for (key, value) in &observed {
                frame.record(*key, *value);
            }
        }
    }
    /// Whether `path` is a definition a reader can look up: indexed, staged
    /// or built in.
    pub(crate) fn names_definition(&self, path: &DefPath) -> bool {
        self.declared(path)
            || self.staged_constants.contains_key(path)
            || self.registry.get(DefId::intern(path)).is_some()
            || self.builtins.contains_key(path)
    }
    /// Every open evaluation frame executed `path`'s body.
    pub(crate) fn read_implementation(&mut self, path: &DefPath) {
        if self.execution_stack.is_empty() {
            return;
        }
        // Only a body that exists can have run.
        let Some(hash) = self.implementation_hash(path) else {
            return;
        };
        let value = memos::optional(Some(hash));
        let key = ReadKey::ExecutedBody(DefId::intern(path));
        for frame in &mut self.execution_stack {
            frame.record(key, value);
        }
    }
    pub(crate) fn merge_mode_execution(&mut self, path: &DefPath) {
        if let Some(entry) = self.modes.get(&DefId::intern(path)).cloned() {
            self.merge_execution(entry.reads());
        }
    }
}
