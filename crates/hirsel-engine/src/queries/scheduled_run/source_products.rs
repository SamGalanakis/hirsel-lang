//! Complete precompute body data from settled source producers. These records
//! carry values and source audits; independent Lower validation retains V authority.
use super::*;
const OUTPUT: Domain = Domain {
    kind: 0x4600_0035,
    schema: 1,
};

#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct Body {
    pub products: crate::native_audit::SourceAuditProducts,
    pub constant: Option<hirsel_tir::Materialized>,
}
impl Body {
    fn weight(&self) -> u64 {
        let mut weight = crate::NativeMetadataWeight::default();
        weight.add(std::mem::size_of::<Self>());
        let audit = &self.products.audit;
        weight.path(&audit.definition);
        weight.flat_vec(&audit.cleanup);
        for (_, allocation) in &audit.cleanup {
            if let hirsel_types::DropAllocation::DependsOn(parameters) = allocation {
                weight.add(std::mem::size_of_val(&**parameters) + 2 * std::mem::size_of::<usize>());
            }
        }
        weight.flat_vec(&audit.closures);
        weight.flat_vec(&audit.invocations);
        weight.vec(&self.products.materialized, |weight, (_, value)| {
            weight.materialized(value)
        });
        if let Some(value) = &self.constant {
            weight.materialized(value);
        }
        weight.bytes()
    }
}
#[derive(serde::Serialize)]
pub(super) struct Packed {
    record: Reference,
    decode_bytes: u64,
}
impl Packed {
    pub(super) fn new(queries: &FrontRunQueries, body: Body) -> Result<Self, QueryError> {
        let retained = body.weight();
        if retained > queries.weight.scratch {
            return Err(operational("source products exceed scratch admission"));
        }
        let (bytes, codec) = queries
            .records
            .encode_with_decode_bytes(&queries.anchors, &body)
            .ok_or(QueryError::NotImplemented)?;
        let decode_bytes = retained
            .checked_mul(2)
            .and_then(|n| n.checked_add(codec))
            .ok_or(QueryError::NotImplemented)?;
        if decode_bytes > queries.weight.scratch || bytes.len() as u64 > queries.weight.scratch {
            return Err(operational(
                "source product codec exceeds scratch admission",
            ));
        }
        let record = queries
            .store
            .put_content(OUTPUT, &bytes)
            .ok_or_else(|| operational("source products exceed transport admission"))?;
        Ok(Self {
            record,
            decode_bytes,
        })
    }
}
/// Bounded decoded source data, with its original type/anchor owners. The
/// caller's grant covers the returned data and its complete decoding workspace.
pub struct FrontSourceProducts {
    body: Body,
    _owner: Arc<FrontRunQueries>,
    _allowance: MemoryReservation,
}
impl FrontSourceProducts {
    pub fn audit(&self) -> &crate::SourceAudit {
        &self.body.products.audit
    }
    pub fn materialized(&self) -> &[(hirsel_syntax::ExprId, hirsel_tir::Materialized)] {
        &self.body.products.materialized
    }
    pub fn constant(&self) -> Option<&hirsel_tir::Materialized> {
        self.body.constant.as_ref()
    }
}
impl FrontRunQueries {
    /// Decode bound of one already completed source product. No provider is read.
    pub fn source_product_bytes(&self, product: ProductRef) -> Result<Option<u64>, QueryError> {
        let entry = self
            .semantic_entry(product)
            .ok_or_else(|| operational("source product is unavailable"))?;
        Ok(entry.source.as_ref().map(|source| source.decode_bytes))
    }
    pub fn source_products_with_allowance(
        self: &Arc<Self>,
        product: ProductRef,
        allowance: MemoryReservation,
    ) -> Result<Option<FrontSourceProducts>, QueryError> {
        let entry = self
            .semantic_entry(product)
            .ok_or_else(|| operational("source product is unavailable"))?;
        let Some(source) = &entry.source else {
            return Ok(None);
        };
        if source.record.domain != OUTPUT
            || source.record.len > allowance.bytes()
            || source.decode_bytes > allowance.bytes()
        {
            return Err(operational(
                "source product decode exceeds caller admission",
            ));
        }
        let bytes = self
            .store
            .get_content(source.record)
            .ok_or(QueryError::NotImplemented)?;
        if OUTPUT.reference(&bytes) != Some(source.record) {
            return Err(operational(
                "source product content differs from its descriptor",
            ));
        }
        let body: Body = self
            .records
            .decode(&self.anchors, &bytes)
            .ok_or(QueryError::NotImplemented)?;
        if body.weight() > allowance.bytes() {
            return Err(QueryError::NotImplemented);
        }
        Ok(Some(FrontSourceProducts {
            body,
            _owner: self.clone(),
            _allowance: allowance,
        }))
    }
}
