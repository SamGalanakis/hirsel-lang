//! Canonical PS9 export is a keyed continuation of the accepted source check.
use super::safe_check::SafeCheck;
use hirsel_engine::{
    CheckOutput, FrontProofExport, FrontProofText, ToolError, runtime::*, store::*,
};
use hirsel_syntax::ContentHash;
use std::{
    rc::Rc,
    sync::{Arc, Mutex},
    task::Poll,
};

pub(super) const KIND: u32 = 0x5200_0020;
const OUTPUT: Domain = Domain {
    kind: 0x5200_0021,
    schema: 1,
};
pub(super) const BYTES: u64 = 4096;
struct Settled {
    product: ProductRef,
    check: CheckOutput,
    text: Result<FrontProofText, String>,
    _copy: Option<MemoryReservation>,
}
pub(super) struct ProofExport {
    query: QueryId,
    safe: Arc<SafeCheck>,
    capture: Arc<FrontProofExport>,
    budget: Arc<ProcessBudget>,
    _allowance: MemoryReservation,
    completed: Mutex<Option<Arc<Settled>>>,
    store: Arc<dyn ResultStore>,
    errors: RuntimeErrors,
}
impl ProofExport {
    pub fn new(
        safe: Arc<SafeCheck>,
        capture: Arc<FrontProofExport>,
        allowance: MemoryReservation,
        budget: Arc<ProcessBudget>,
        store: Arc<dyn ResultStore>,
        errors: RuntimeErrors,
    ) -> Arc<Self> {
        Arc::new(Self {
            query: QueryId {
                kind: KIND,
                address: Address(ContentHash::of_parts(
                    "hirsel.proof-export.address.v1",
                    [
                        safe.query().address.0.0.as_slice(),
                        capture.query().address.0.0.as_slice(),
                        hirsel_proof_export::FORMAT.as_bytes(),
                    ],
                )),
            },
            safe,
            capture,
            budget,
            _allowance: allowance,
            completed: Mutex::new(None),
            store,
            errors,
        })
    }
    pub fn query(&self) -> QueryId {
        self.query
    }
    pub fn reusable(&self) -> bool {
        self.completed
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .is_none_or(|settled| {
                settled.text.is_ok()
                    || settled.check.completion != hirsel_engine::CheckCompletion::Complete
                    || settled
                        .check
                        .diagnostics
                        .iter()
                        .any(|diagnostic| diagnostic.severity == hirsel_diag::Severity::Error)
            })
    }
    pub fn factory(self: &Arc<Self>) -> Arc<dyn QueryFactory> {
        Arc::new(Factory(self.clone()))
    }
    pub fn outcome(
        &self,
        product: ProductRef,
    ) -> Result<(CheckOutput, Result<Vec<u8>, String>), ToolError> {
        let settled = self
            .completed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .filter(|s| s.product == product)
            .ok_or_else(|| ToolError::Runtime("proof export product is unavailable".into()))?;
        Ok((
            settled.check.clone(),
            settled
                .text
                .as_ref()
                .map(|text| text.text().as_bytes().to_vec())
                .map_err(Clone::clone),
        ))
    }
    fn build(&self, product: ProductRef) -> Result<(FrontProofText, MemoryReservation), String> {
        let stage = |bytes, name| {
            super::host_selector::GraphBudget::reserve_product(&self.budget, bytes).map_err(|error| {
                format!("proof export {name} needs {bytes} bytes from the shared process budget: {error}")
            })
        };
        let error = |e| format!("proof export: {e:?}");
        let input = Rc::new(
            self.capture
                .input_with_allowance(
                    product.clone(),
                    stage(self.capture.input_bytes(product).map_err(error)?, "input")?,
                )
                .map_err(error)?,
        );
        let bytes = input.type_capture_bytes().map_err(error)?;
        let prepared = input
            .capture_types_with_allowance(stage(bytes, "type capture")?)
            .map_err(error)?;
        let bytes = prepared.owned_build_bytes().map_err(error)?;
        let built = prepared
            .build_owned_with_allowance(stage(bytes, "construction")?)
            .map_err(error)?;
        let bytes = built.output_bytes().map_err(error)?;
        let text = built
            .encode_with_allowance(stage(bytes, "output")?)
            .map_err(error)?;
        let copy = stage(text.text().len() as u64, "readiness output")?;
        Ok((text, copy))
    }
}
struct Factory(Arc<ProofExport>);
impl QueryFactory for Factory {
    fn create(&self, query: QueryId) -> Result<Box<dyn QueryTask>, Reference> {
        if query != self.0.query {
            return Err(self.0.errors.protocol);
        }
        Ok(Box::new(Task(self.0.clone())))
    }
}
struct Task(Arc<ProofExport>);
impl QueryTask for Task {
    fn id(&self) -> QueryId {
        self.0.query
    }
    fn weight(&self) -> MemoryWeight {
        MemoryWeight {
            retained: BYTES,
            scratch: 0,
            stack: 0,
        }
    }
    fn poll(&mut self, cx: &mut dyn QueryContext) -> QueryPoll {
        let owner = &self.0;
        if let Some(settled) = owner
            .completed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            return QueryPoll::Complete(settled.product.clone());
        }
        let product = match cx.read(owner.safe.query()) {
            Poll::Pending => return QueryPoll::Waiting(vec![owner.safe.query()]),
            Poll::Ready(Err(error)) => return QueryPoll::Failed(error),
            Poll::Ready(Ok(product)) => product,
        };
        let (check, run) = match owner.safe.settled(product.clone()) {
            Ok(output) => output,
            Err(_) => return QueryPoll::Failed(owner.errors.protocol),
        };
        let built = run
            .ok_or_else(|| "check was not accepted".to_owned())
            .and_then(|run| owner.build(run));
        let descriptor = match &built {
            Ok((text, _)) => ContentHash::of_bytes(text.text().as_bytes()).0.to_vec(),
            Err(error) => error.as_bytes().to_vec(),
        };
        let Ok(input_archive) = product.archive(ArchiveAdmission::Retained) else {
            return QueryPoll::Failed(owner.errors.resource);
        };
        let mut bytes = input_archive.reference().output.0.0.to_vec();
        bytes.extend_from_slice(&descriptor);
        let Some(record) = owner.store.put_content(OUTPUT, &bytes) else {
            return QueryPoll::Failed(owner.errors.resource);
        };
        let action = Action(ContentHash::of_parts(
            "hirsel.proof-export.action.v1",
            [
                owner.query.address.0.0.as_slice(),
                record.output.0.0.as_slice(),
            ],
        ));
        let Some(archive) = owner.store.open_product_archive(record) else {
            return QueryPoll::Failed(owner.errors.resource);
        };
        let product = ProductRef::import_archive(owner.query.address, action, archive);
        let (text, copy) = match built {
            Ok((text, copy)) => (Ok(text), Some(copy)),
            Err(error) => (Err(error), None),
        };
        *owner.completed.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(Settled {
            product: product.clone(),
            check,
            text,
            _copy: copy,
        }));
        owner
            .store
            .associate_product(product.address, action, record);
        QueryPoll::Complete(product)
    }
}
