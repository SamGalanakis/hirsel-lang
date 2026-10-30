use super::{
    ArchiveAdmission, LiveAnalyzedProduct, LiveLowerProduct, LiveSummaryProduct, ProductArchive,
};
use hirsel_artifact_store::{Action, Address, Domain};
use std::{fmt, io};

/// An immutable publication retains its concrete value and admission owners.
/// Portable bytes are obtained only by explicitly materializing its archive.
#[derive(Clone)]
pub struct ProductRef {
    pub address: Address,
    pub action: Action,
    value: Value,
}

#[derive(Clone)]
enum Value {
    Summary(LiveSummaryProduct),
    Analyzed(LiveAnalyzedProduct),
    Lower(LiveLowerProduct),
    Imported(ProductArchive),
}

impl ProductRef {
    pub fn summary(value: LiveSummaryProduct) -> Self {
        Self {
            address: value.address(),
            action: value.action(),
            value: Value::Summary(value),
        }
    }
    pub fn analyzed(value: LiveAnalyzedProduct) -> Self {
        Self {
            address: value.address(),
            action: value.action(),
            value: Value::Analyzed(value),
        }
    }
    pub fn lower(value: LiveLowerProduct) -> Self {
        Self {
            address: value.address(),
            action: value.action(),
            value: Value::Lower(value),
        }
    }
    /// Import a fully checked archive whose complete backing is already admitted.
    pub fn import_archive(address: Address, action: Action, archive: ProductArchive) -> Self {
        Self {
            address,
            action,
            value: Value::Imported(archive),
        }
    }
    pub fn domain(&self) -> Domain {
        match &self.value {
            Value::Summary(value) => value.domain(),
            Value::Analyzed(value) => value.domain(),
            Value::Lower(value) => value.domain(),
            Value::Imported(archive) => archive.reference().domain,
        }
    }
    pub fn summary_value(&self) -> Option<&LiveSummaryProduct> {
        match &self.value {
            Value::Summary(value) => Some(value),
            _ => None,
        }
    }
    pub fn analyzed_value(&self) -> Option<&LiveAnalyzedProduct> {
        match &self.value {
            Value::Analyzed(value) => Some(value),
            _ => None,
        }
    }
    pub fn lower_value(&self) -> Option<&LiveLowerProduct> {
        match &self.value {
            Value::Lower(value) => Some(value),
            _ => None,
        }
    }
    /// Retained admission consumes only an existing archive. A new encoding
    /// requires the boundary's explicit working partition. Failure is retryable.
    pub fn archive(&self, admission: ArchiveAdmission<'_>) -> io::Result<ProductArchive> {
        match &self.value {
            Value::Summary(value) => value.archive_with(admission),
            Value::Analyzed(value) => value.archive_with(admission),
            Value::Lower(value) => value.archive_with(admission),
            Value::Imported(archive) => Ok(archive.clone()),
        }
    }
}

impl PartialEq for ProductRef {
    fn eq(&self, other: &Self) -> bool {
        if self.address != other.address || self.action != other.action {
            return false;
        }
        match (&self.value, &other.value) {
            (Value::Summary(a), Value::Summary(b)) => a.same_owner(b),
            (Value::Analyzed(a), Value::Analyzed(b)) => a.same_owner(b),
            (Value::Lower(a), Value::Lower(b)) => a.same_owner(b),
            (Value::Imported(a), Value::Imported(b)) => a.reference() == b.reference(),
            _ => false,
        }
    }
}
impl Eq for ProductRef {}
impl fmt::Debug for ProductRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProductRef")
            .field("address", &self.address)
            .field("action", &self.action)
            .field("domain", &self.domain())
            .finish()
    }
}
