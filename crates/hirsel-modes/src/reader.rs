//! The signature reads of one mode query. Every read, including those a
//! `ContextMemo` proof makes or replays on a hit, goes through one `Reader`,
//! which asks the provider once per definition and queues each first read for
//! the pass to record (its dependency, diagnostics or failure).
use hirsel_syntax::{DefId, DefMap};
use hirsel_types::{QueryError, Signature, SignatureProvider, SignatureRef};
use std::cell::RefCell;
use std::sync::Arc;

pub struct Reader<'p> {
    provider: &'p mut dyn SignatureProvider,
    own: (DefId, SignatureRef),
    cache: DefMap<Result<SignatureRef, QueryError>>,
    /// First reads the pass has not recorded yet, in read order.
    pending: Vec<(DefId, Result<SignatureRef, QueryError>)>,
}
impl<'p> Reader<'p> {
    pub fn new(provider: &'p mut dyn SignatureProvider, own: &Signature) -> RefCell<Self> {
        RefCell::new(Self {
            provider,
            own: (own.definition, Arc::new(own.clone())),
            cache: DefMap::default(),
            pending: vec![],
        })
    }
    /// The checked definition's own signature is read without a dependency.
    pub fn read(&mut self, definition: DefId) -> Result<SignatureRef, QueryError> {
        if definition == self.own.0 {
            return Ok(self.own.1.clone());
        }
        if let Some(result) = self.cache.get(&definition) {
            return result.clone();
        }
        let result = self.provider.signature(definition);
        self.cache.insert(definition, result.clone());
        self.pending.push((definition, result.clone()));
        result
    }
    /// A read made earlier, if any; it makes no new read.
    pub fn cached(&self, definition: DefId) -> Option<SignatureRef> {
        if definition == self.own.0 {
            return Some(self.own.1.clone());
        }
        self.cache.get(&definition)?.as_ref().ok().cloned()
    }
    /// First reads since the last call, for the pass to record.
    pub fn take_pending(&mut self) -> Vec<(DefId, Result<SignatureRef, QueryError>)> {
        std::mem::take(&mut self.pending)
    }
    /// Every signature read successfully, in definition order.
    pub fn signatures(&self) -> Vec<SignatureRef> {
        let mut read: Vec<_> = self
            .cache
            .iter()
            .filter_map(|(d, s)| Some((*d, s.as_ref().ok()?.clone())))
            .collect();
        read.sort_by(|(a, _), (b, _)| a.path().cmp(b.path()));
        read.into_iter().map(|(_, s)| s).collect()
    }
    pub fn provider(&mut self) -> &mut dyn SignatureProvider {
        self.provider
    }
}
