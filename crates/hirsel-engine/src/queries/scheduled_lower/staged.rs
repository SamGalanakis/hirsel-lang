//! Size selections use the ordinary Evaluate factory and descriptor store.
//! The declaration and actuals determine a distinct immutable query identity.
use super::*;
#[derive(Clone)]
pub(super) struct Selection {
    pub declaration: DefId,
    pub owner: DefId,
    pub arguments: Vec<u64>,
    pub instance: DefId,
    pub key: EntryKey,
}
pub(super) struct Staged {
    pub declarations: BTreeMap<DefId, crate::comptime::StagedConstant>,
    selections: Mutex<BTreeMap<ContentHash, Selection>>,
}
impl Staged {
    pub(super) fn new(declarations: &BTreeMap<DefPath, crate::comptime::StagedConstant>) -> Self {
        Self {
            declarations: declarations
                .iter()
                .map(|(path, row)| (DefId::intern(path), row.clone()))
                .collect(),
            selections: Mutex::new(BTreeMap::new()),
        }
    }
    pub(super) fn retained_bytes(&self) -> u64 {
        let mut weight = crate::NativeMetadataWeight::default();
        weight.tree::<DefId, crate::comptime::StagedConstant>(self.declarations.len());
        for row in self.declarations.values() {
            weight.path(&row.owner);
            weight.flat_vec(&row.inputs);
        }
        weight.bytes()
    }
    pub(super) fn selection(&self, id: QueryId) -> Option<Selection> {
        if id.kind != FRONT_EVALUATE_KIND {
            return None;
        }
        self.selections
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(&id.address.0)
            .cloned()
    }
    pub(super) fn selected(&self, path: &DefPath) -> Option<QueryId> {
        self.selections
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .find(|(_, row)| row.instance.path() == path)
            .map(|(hash, _)| QueryId {
                kind: FRONT_EVALUATE_KIND,
                address: hirsel_artifact_store::Address(*hash),
            })
    }
    pub(super) fn instances(&self) -> Vec<DefPath> {
        self.selections
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .values()
            .map(|row| row.instance.path().clone())
            .collect()
    }
}
impl FrontLowerQueries {
    pub(super) fn staged_query(
        &self,
        path: &DefPath,
        arguments: &[u64],
    ) -> Result<QueryId, QueryError> {
        let id = DefId::intern(path);
        let declaration = self
            .staged
            .declarations
            .get(&id)
            .ok_or(QueryError::NotImplemented)?;
        if arguments.len() != declaration.inputs.len() {
            return Err(QueryError::NotImplemented);
        }
        let owner = DefId::intern(&declaration.owner);
        let original = &self.inputs[owner.path()].evaluate;
        let bytes =
            bincode::serialize(&(path, arguments)).map_err(|_| QueryError::NotImplemented)?;
        let hash = ContentHash::of_parts(
            "hirsel.front.staged-evaluate.v1",
            [original.0.0.as_slice(), bytes.as_slice()],
        );
        let query = QueryId {
            kind: FRONT_EVALUATE_KIND,
            address: hirsel_artifact_store::Address(hash),
        };
        let mut selections = self
            .staged
            .selections
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !selections.contains_key(&hash) {
            let mut weight = crate::NativeMetadataWeight::default();
            let mut previous = crate::NativeMetadataWeight::default();
            weight.tree::<ContentHash, Selection>(selections.len() + 1);
            if !selections.is_empty() {
                previous.tree::<ContentHash, Selection>(selections.len());
            }
            let required = weight
                .bytes()
                .checked_sub(previous.bytes())
                .and_then(|bytes| bytes.checked_add(size_of_val(arguments) as u64))
                .ok_or(QueryError::NotImplemented)?;
            let mut remaining = self
                .retention
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            *remaining = remaining
                .checked_sub(required)
                .ok_or(QueryError::NotImplemented)?;
            selections.insert(
                hash,
                Selection {
                    declaration: id,
                    owner,
                    arguments: arguments.to_vec(),
                    instance: DefId::intern(&hirsel_tir::constant_instance_path(path, arguments)),
                    key: EntryKey(hash, original.1, QueryKind::Evaluate),
                },
            );
        }
        Ok(query)
    }
}
