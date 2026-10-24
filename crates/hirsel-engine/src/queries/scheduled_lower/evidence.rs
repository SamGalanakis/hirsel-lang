//! Opaque private execution evidence, following S1 TaskWorker's acceptance
//! composition. Missing evidence disables reuse; it never disables actual V.
use crate::{ReadKey, Reads, Verify};
use hirsel_validate::{AcceptedArtifact, ExecutionReceipt};
pub(super) struct Evidence {
    pub(super) executed: bool,
    proofs: Option<Vec<AcceptedArtifact>>,
    max_proofs: usize,
}
impl Evidence {
    pub(super) fn new(verify: Verify, bytes: u64) -> Self {
        Self {
            executed: false,
            proofs: (verify == Verify::On).then(Vec::new),
            max_proofs: (bytes / (std::mem::size_of::<AcceptedArtifact>() as u64 + 32))
                .try_into()
                .unwrap_or(usize::MAX),
        }
    }
    pub(super) fn accepted(&mut self, proof: Option<AcceptedArtifact>) {
        let Some(proof) = proof else {
            self.proofs = None;
            return;
        };
        let Some(proofs) = &mut self.proofs else {
            return;
        };
        if !proofs.contains(&proof) {
            if proofs.len() == self.max_proofs {
                self.proofs = None;
            } else {
                proofs.push(proof);
            }
        }
    }
    pub(super) fn dependency(&mut self, executed: bool, receipt: Option<&ExecutionReceipt>) {
        if !executed {
            return;
        }
        self.executed = true;
        if let Some(receipt) = receipt {
            for proof in receipt.artifacts() {
                self.accepted(Some(proof.clone()));
            }
        } else {
            self.proofs = None;
        }
    }
    pub(super) fn finish(&self, reads: &Reads) -> Option<ExecutionReceipt> {
        if !self.executed {
            return None;
        }
        let proofs = self.proofs.as_ref()?;
        let mut contexts: Vec<_> = reads
            .iter()
            .filter_map(|(key, digest)| match key {
                ReadKey::ValidationUniverse(package) => Some((package.id(), *digest)),
                _ => None,
            })
            .collect();
        contexts.sort_unstable();
        ExecutionReceipt::compose(&contexts, proofs)
    }
}
