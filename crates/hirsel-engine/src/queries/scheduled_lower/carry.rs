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
    verified: Option<StoredVerification>,
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
    constant_decode_bytes: u64,
    reads_decode_bytes: u64,
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
    pub(super) fn observe(&self, owner: &DefPath, read: &ReadKey) -> Option<ContentHash> {
        match *read {
            ReadKey::ExecutedBody(id) => Some(memos::optional(
                self.inputs.get_id(id).map(|input| input.implementation),
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
                [
                    Some((crate::NATIVE_ORIGINAL_BODY, lower.body)),
                    Some((LOWER_MATERIALIZED, lower.materialized)),
                    lower
                        .verified
                        .as_ref()
                        .map(|stored| (LOWER_VERIFICATION, stored.metadata)),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>(),
            ),
            Ok(Value::Constant(..)) => (None, vec![]),
            Err(_) => return,
        };
        let mut weight = crate::NativeMetadataWeight::default();
        // At most one full internal node per row bounds every BTree split.
        weight.tree::<(u32, ContentHash), Row>(0);
        weight.add(size_of::<Row>() + descriptor.len());
        for (_, bytes) in outcome.reads.allocations() {
            weight.add(bytes);
        }
        if let Some(receipt) = &outcome.execution {
            let Some(bytes) = receipt.retained_bytes() else {
                return;
            };
            weight.add(bytes.try_into().unwrap_or(usize::MAX));
        }
        let mut contents = Vec::new();
        for (domain, reference) in references {
            weight.add(size_of::<(Domain, Arc<[u8]>)>() + 2 * size_of::<usize>());
            weight.add(reference.len.try_into().unwrap_or(usize::MAX));
            if weight.bytes() > carry.allowance.bytes() {
                return;
            }
            let Some(bytes) = self.store.get_content(reference) else {
                return;
            };
            contents.push((domain, Arc::<[u8]>::from(&*bytes)));
        }
        // Ordinary records refer to canonical type components outside their
        // payload. Carry those components too, including their transitive edges.
        // Otherwise a fresh graph publishes a body it cannot decode.
        let mut types = BTreeMap::new();
        for bytes in
            std::iter::once(descriptor).chain(contents.iter().filter_map(|(domain, bytes)| {
                (*domain != LOWER_VERIFICATION).then_some(bytes.as_ref())
            }))
        {
            let captured = self.records.visit_record_types(bytes, |reference| {
                if types.contains_key(&reference.output.0) {
                    return Some(false);
                }
                weight.add(size_of::<(Domain, Arc<[u8]>)>() + 2 * size_of::<usize>());
                weight.add(reference.len.try_into().ok()?);
                // Bound both retained bytes and the temporary deduplication map.
                let mut peak = crate::NativeMetadataWeight::default();
                peak.tree::<ContentHash, Arc<[u8]>>(types.len() + 1);
                if weight.bytes().saturating_add(peak.bytes()) > carry.allowance.bytes() {
                    return None;
                }
                let bytes = self.store.get_content(reference)?;
                types.insert(reference.output.0, Arc::<[u8]>::from(&*bytes));
                Some(true)
            });
            if captured.is_none() {
                return;
            }
        }
        contents.extend(
            types
                .into_values()
                .map(|bytes| (hirsel_product_codec::TypeRecords::DOMAIN, bytes)),
        );
        let charge = weight.bytes();
        carry.remember(
            (id.kind, id.address.0),
            Row {
                text: key.1,
                reads: outcome.reads.clone(),
                descriptor: descriptor.into(),
                contents: contents.into_boxed_slice(),
                lower,
                constant_decode_bytes: match &outcome.result {
                    Ok(Value::Constant(value)) => value.working_bytes(),
                    _ => 0,
                },
                reads_decode_bytes: outcome.reads_decode_bytes,
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
                archived_metadata: None,
                verified: retained.verified,
                acceptance: retained.acceptance,
                materialized,
                definition_decode_layout: retained.definition_decode_layout,
                constant_selection_bytes: retained.constant_selection_bytes,
            };
            (Value::Lower(Box::new(lower)), reads, depth, executed)
        } else {
            if row.constant_decode_bytes > self.weight.scratch {
                return None;
            }
            let (value, substitutions, reads, depth, executed): (
                Materialized,
                Vec<(hirsel_syntax::ExprId, Materialized)>,
                Reads,
                DepthEvidence<Span>,
                bool,
            ) = self.records.decode(&self.anchors, &row.descriptor)?;
            let mut constants = Constants::new(value, substitutions);
            constants.set_working_bytes(row.constant_decode_bytes);
            (Value::Constant(Box::new(constants)), reads, depth, executed)
        };
        let (value, reads, depth, executed) = result;
        if reads.len() != row.reads.len() {
            return None;
        }
        let reads_decode_bytes = row.reads_decode_bytes;
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
                reads_decode_bytes,
                depth: depth.into(),
                executed,
                execution,
                execution_subject: persistent::evaluation_subject(key, &descriptor),
            },
            false,
        ))
    }
}
