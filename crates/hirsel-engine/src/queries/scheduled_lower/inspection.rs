use super::*;

/// One ordered dependency sequence decoded under its caller's retained grant.
pub struct FrontLowerDependencies {
    dependencies: Vec<SignatureDependency>,
    _owner: Arc<FrontLowerQueries>,
    _allowance: MemoryReservation,
}
impl FrontLowerDependencies {
    pub fn dependencies(&self) -> &[SignatureDependency] {
        &self.dependencies
    }
}

impl LowerProduct {
    fn dependency_decode_bytes(&self) -> Option<u64> {
        let header = size_of::<FrontLowerDependencies>() as u64;
        match &self.archived_metadata {
            Some(archive) => header
                .checked_add(archive.reference.len)?
                .checked_add(archive.decode_bytes),
            None => header.checked_add(
                self.dependencies
                    .len()
                    .checked_mul(size_of::<SignatureDependency>())?
                    .try_into()
                    .ok()?,
            ),
        }
    }
}

impl FrontLowerQueries {
    /// Read selected source rows without reconstructing a Definition or Body.
    /// The caller's grant and this exact source generation survive every view.
    pub fn settled_tir(
        self: &Arc<Self>,
        path: &DefPath,
        allowance: MemoryReservation,
    ) -> Result<crate::NativeOriginalValue, QueryError> {
        let id = self.query(path).ok_or(QueryError::NotImplemented)?;
        let outcomes = self
            .outcomes
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let (_, outcome, _) = outcomes
            .get(&(id.kind, id.address.0))
            .ok_or(QueryError::NotImplemented)?;
        let lower = match &outcome.result {
            Ok(Value::Lower(lower)) => lower,
            Err(error) => return Err(error.clone()),
            _ => return Err(QueryError::NotImplemented),
        };
        if lower
            .definition_decode_layout
            .total()
            .ok_or(QueryError::NotImplemented)?
            > allowance.bytes()
        {
            return Err(QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "original row inspection".into(),
                message: "captured decode buffers exceed caller working admission".into(),
            }));
        }
        if let Some(live) = lower.live.as_ref().and_then(WeakLowerProduct::upgrade) {
            return Ok(crate::NativeOriginalValue::Live(live.clone()));
        }
        let reference = lower.body;
        drop(outcomes);
        let archive = self
            .records
            .open_tir(
                Arc::new(self.anchors.clone()),
                self.store
                    .get_content(reference)
                    .ok_or(QueryError::NotImplemented)?,
                reference,
                Arc::new((self.clone(), allowance)),
            )
            .ok_or(QueryError::NotImplemented)?;
        if archive
            .header()
            .ok_or(QueryError::NotImplemented)?
            .path
            .path()
            != path
        {
            return Err(QueryError::NotImplemented);
        }
        Ok(crate::NativeOriginalValue::Archive(archive))
    }
    /// Captured bound for an already settled ordered dependency sequence.
    /// This inspects metadata only, before any provider access or decoding.
    pub fn settled_dependency_decode_bytes(&self, path: &DefPath) -> Result<u64, QueryError> {
        let id = self.query(path).ok_or(QueryError::NotImplemented)?;
        let outcomes = self
            .outcomes
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let (_, outcome, _) = outcomes
            .get(&(id.kind, id.address.0))
            .ok_or(QueryError::NotImplemented)?;
        match &outcome.result {
            Ok(Value::Lower(lower)) => lower
                .dependency_decode_bytes()
                .ok_or(QueryError::NotImplemented),
            Err(error) => Err(error.clone()),
            _ => Err(QueryError::NotImplemented),
        }
    }
    /// Read only settled dependency metadata. The output owns the working grant
    /// and source owner through its last borrowed use; no query is requested.
    pub fn settled_dependencies(
        self: &Arc<Self>,
        path: &DefPath,
        allowance: MemoryReservation,
    ) -> Result<FrontLowerDependencies, QueryError> {
        let id = self.query(path).ok_or(QueryError::NotImplemented)?;
        let outcomes = self
            .outcomes
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let (_, outcome, _) = outcomes
            .get(&(id.kind, id.address.0))
            .ok_or(QueryError::NotImplemented)?;
        let lower = match &outcome.result {
            Ok(Value::Lower(lower)) => lower,
            Err(error) => return Err(error.clone()),
            _ => return Err(QueryError::NotImplemented),
        };
        if lower
            .dependency_decode_bytes()
            .ok_or(QueryError::NotImplemented)?
            > allowance.bytes()
        {
            return Err(QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "original dependency inspection".into(),
                message: "captured decode buffers exceed caller working admission".into(),
            }));
        }
        if let Some(live) = lower.live.as_ref().and_then(WeakLowerProduct::upgrade) {
            return Ok(FrontLowerDependencies {
                dependencies: live.output().dependencies.clone(),
                _owner: self.clone(),
                _allowance: allowance,
            });
        }
        let archive = match &lower.archived_metadata {
            Some(archive) => archive.clone(),
            None => {
                return Ok(FrontLowerDependencies {
                    dependencies: lower.dependencies.to_vec(),
                    _owner: self.clone(),
                    _allowance: allowance,
                });
            }
        };
        drop(outcomes);
        let dependencies = archive
            .dependencies(
                &self.records,
                &self.anchors,
                &*self.store,
                allowance.bytes(),
            )
            .ok_or(QueryError::NotImplemented)?;
        Ok(FrontLowerDependencies {
            dependencies,
            _owner: self.clone(),
            _allowance: allowance,
        })
    }
}

impl FrontLowerQueries {
    pub fn settled_checked_dependency_decode_bytes(
        &self,
        path: &DefPath,
    ) -> Result<u64, QueryError> {
        let output = self
            .front
            .checked_definition(path)
            .ok_or(QueryError::NotImplemented)?;
        (size_of::<FrontLowerDependencies>() as u64)
            .checked_add((output.dependencies.len() * size_of::<SignatureDependency>()) as u64)
            .ok_or(QueryError::NotImplemented)
    }
    pub fn settled_checked_dependencies(
        self: &Arc<Self>,
        path: &DefPath,
        allowance: MemoryReservation,
    ) -> Result<FrontLowerDependencies, QueryError> {
        if self.settled_checked_dependency_decode_bytes(path)? > allowance.bytes() {
            return Err(QueryError::Operational(hirsel_types::OperationalFailure {
                operation: "checked dependency inspection".into(),
                message: "captured ordered dependencies exceed caller working admission".into(),
            }));
        }
        let output = self
            .front
            .checked_definition(path)
            .ok_or(QueryError::NotImplemented)?;
        Ok(FrontLowerDependencies {
            dependencies: output
                .dependencies
                .values()
                .ok_or(QueryError::NotImplemented)?,
            _owner: self.clone(),
            _allowance: allowance,
        })
    }
}
