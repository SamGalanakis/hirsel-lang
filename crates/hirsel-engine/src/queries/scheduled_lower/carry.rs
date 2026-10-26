//! Bounded accepted Lower/Evaluate outcomes across fresh source graphs. The
//! ordinary type/anchor codec relocates each descriptor and its stored body and
//! materialized constants; V evidence names content only and is kept as
//! captured. A row is adopted only while its input text and every read still
//! observe their recorded values, so a later graph never lowers it again.
use super::*;

/// What the descriptor bytes do not encode: content-addressed V evidence and
/// layout facts measured when the body was lowered.
#[derive(Clone)]
struct Retained {
    verified: Option<hirsel_validate::VerifiedDefinitionOutput>,
    acceptance: Option<hirsel_validate::AcceptedArtifact>,
    definition_decode_layout: DefinitionDecodeLayout,
    constant_selection_bytes: u64,
}
struct Row {
    text: ContentHash,
    reads: Reads,
    descriptor: Arc<[u8]>,
    /// Store content the descriptor names: a Lower body and its constants.
    contents: Box<[(Domain, Arc<[u8]>)]>,
    lower: Option<Retained>,
    execution: Option<hirsel_validate::ExecutionReceipt>,
    charge: u64,
}
#[derive(Default)]
struct Rows {
    entries: BTreeMap<(u32, ContentHash), Row>,
    used: u64,
}
/// Optional host-owned carry, independently bounded from ordinary storage.
pub struct LowerCarry {
    rows: Mutex<Rows>,
    allowance: MemoryReservation,
}
impl LowerCarry {
    pub fn new(allowance: MemoryReservation) -> Result<Arc<Self>, MemoryReservation> {
        let header = size_of::<Self>() as u64 + 2 * size_of::<usize>() as u64;
        if allowance.bytes() < header {
            return Err(allowance);
        }
        Ok(Arc::new(Self {
            rows: Mutex::new(Rows {
                used: header,
                ..Rows::default()
            }),
            allowance,
        }))
    }
    pub fn retained_bytes(&self) -> u64 {
        self.rows.lock().unwrap_or_else(|e| e.into_inner()).used
    }
    pub fn len(&self) -> usize {
        self.rows
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entries
            .len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn remember(&self, key: (u32, ContentHash), row: Row) {
        let mut rows = self.rows.lock().unwrap_or_else(|e| e.into_inner());
        let old = rows.entries.get(&key).map_or(0, |r| r.charge);
        let Some(used) = rows
            .used
            .checked_sub(old)
            .and_then(|n| n.checked_add(row.charge))
        else {
            return;
        };
        if used > self.allowance.bytes() {
            return;
        }
        rows.entries.insert(key, row);
        rows.used = used;
    }
}
impl FrontLowerQueries {
    /// Install before submitting tasks. The caller keeps the same carry for
    /// later fresh graphs; it holds neither this producer nor its sources.
    pub fn install_carry(&self, carry: Arc<LowerCarry>) -> Result<(), Arc<LowerCarry>> {
        let mut slot = self.carry.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_some()
            || !self
                .outcomes
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_empty()
        {
            return Err(carry);
        }
        *slot = Some(carry);
        Ok(())
    }
    fn carry(&self) -> Option<Arc<LowerCarry>> {
        self.carry.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    /// A read's value in this graph. Body and layout texts come from this
    /// producer's own inputs; every other read is the front's source fact.
    fn observe(&self, owner: &DefPath, read: &ReadKey) -> Option<ContentHash> {
        match *read {
            ReadKey::ExecutedBody(id) => Some(memos::optional(
                self.inputs.get(id.path()).map(|input| input.implementation),
            )),
            ReadKey::LayoutBody(id) => Some(memos::optional(self.layouts.get(&id).copied())),
            _ => self.front.observe(owner, read),
        }
    }
    /// Keep one successful outcome for later graphs, with the store content
    /// its descriptor names. A full carry keeps its existing rows.
    pub(super) fn remember_carry(
        &self,
        id: QueryId,
        key: &EntryKey,
        descriptor: &[u8],
        outcome: &Outcome,
    ) {
        let Some(carry) = self.carry() else {
            return;
        };
        let (lower, references) = match &outcome.result {
            Ok(Value::Lower(lower)) => (
                Some(Retained {
                    verified: lower.verified.clone(),
                    acceptance: lower.acceptance.clone(),
                    definition_decode_layout: lower.definition_decode_layout,
                    constant_selection_bytes: lower.constant_selection_bytes,
                }),
                vec![
                    (crate::NATIVE_ORIGINAL_BODY, lower.body),
                    (LOWER_MATERIALIZED, lower.materialized),
                ],
            ),
            Ok(Value::Constant(..)) => (None, vec![]),
            Err(_) => return,
        };
        let Some(contents) = references
            .into_iter()
            .map(|(domain, reference)| {
                let bytes = self.store.get_content(reference)?;
                Some((domain, Arc::<[u8]>::from(&*bytes)))
            })
            .collect::<Option<Box<[_]>>>()
        else {
            return;
        };
        let mut weight = crate::NativeMetadataWeight::default();
        // At most one full internal node per row bounds every BTree split.
        weight.tree::<(u32, ContentHash), Row>(0);
        weight.add(size_of::<Row>() + descriptor.len());
        for (_, bytes) in &contents {
            weight.add(size_of::<(Domain, Arc<[u8]>)>() + 2 * size_of::<usize>() + bytes.len());
        }
        for (_, bytes) in outcome.reads.allocations() {
            weight.add(bytes);
        }
        let mut charge = weight.bytes();
        if let Some(verified) = lower.as_ref().and_then(|l| l.verified.as_ref()) {
            let Some(bytes) = verified.retained_bytes() else {
                return;
            };
            charge = charge.saturating_add(bytes);
        }
        if let Some(receipt) = &outcome.execution {
            let Some(bytes) = receipt.retained_bytes() else {
                return;
            };
            charge = charge.saturating_add(bytes);
        }
        carry.remember(
            (id.kind, id.address.0),
            Row {
                text: key.1,
                reads: outcome.reads.clone(),
                descriptor: descriptor.into(),
                contents,
                lower,
                execution: outcome.execution.clone(),
                charge,
            },
        );
    }
    /// The carried outcome of this query, adopted when its text and reads are
    /// current. Its content returns to this graph's store before publication.
    pub(super) fn load_carry(
        &self,
        id: QueryId,
        path: &DefPath,
        key: &EntryKey,
    ) -> Option<Result<ProductRef, Reference>> {
        let carry = self.carry()?;
        let rows = carry.rows.lock().unwrap_or_else(|e| e.into_inner());
        let row = rows.entries.get(&(id.kind, id.address.0))?;
        if row.text != key.1
            || row.descriptor.len() as u64 > self.weight.scratch
            || row
                .reads
                .first_changed(|read| self.observe(path, read))
                .is_some()
        {
            return None;
        }
        for (domain, bytes) in &row.contents {
            self.store.put_content(*domain, bytes)?;
        }
        let result = if id.kind == FRONT_LOWER_KIND {
            let retained = row.lower.clone()?;
            let (definition, body, diagnostics, dependencies, materialized, reads, depth, executed): (
                DefId,
                Reference,
                Vec<Diagnostic>,
                Vec<SignatureDependency>,
                Reference,
                Reads,
                DepthEvidence<Span>,
                bool,
            ) = self.records.decode(&self.anchors, &row.descriptor)?;
            let lower = LowerProduct {
                definition,
                body,
                diagnostics,
                dependencies: dependencies.into(),
                verified: retained.verified,
                acceptance: retained.acceptance,
                materialized,
                definition_decode_layout: retained.definition_decode_layout,
                constant_selection_bytes: retained.constant_selection_bytes,
            };
            (Value::Lower(lower), reads, depth, executed)
        } else {
            let (value, substitutions, reads, depth, executed): (
                Materialized,
                Vec<(hirsel_syntax::ExprId, Materialized)>,
                Reads,
                DepthEvidence<Span>,
                bool,
            ) = self.records.decode(&self.anchors, &row.descriptor)?;
            (
                Value::Constant(Constants::new(value, substitutions)),
                reads,
                depth,
                executed,
            )
        };
        let (value, reads, depth, executed) = result;
        if reads.len() != row.reads.len() {
            return None;
        }
        let execution = row.execution.clone();
        let descriptor = row.descriptor.clone();
        drop(rows);
        let domain = if id.kind == FRONT_LOWER_KIND {
            LOWER_DESCRIPTOR
        } else {
            CONSTANT_DESCRIPTOR
        };
        Some(self.publish(
            id,
            key,
            domain,
            &descriptor,
            Outcome {
                result: Ok(value),
                reads,
                depth,
                executed,
                execution,
                execution_subject: persistent::evaluation_subject(key, &descriptor),
            },
        ))
    }
}
