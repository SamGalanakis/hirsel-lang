//! Runtime calls close staged size actuals only after the ordered Check root.
use super::*;

#[derive(Default)]
pub(super) struct RuntimeStages {
    selections: Option<Vec<super::super::scheduled_lower::staged_runtime::Selection>>,
    index: usize,
    pub(super) waiting: Option<QueryId>,
}
impl RuntimeStages {
    pub(super) fn poll(
        &mut self,
        queries: &FrontRunQueries,
        cx: &mut dyn QueryContext,
    ) -> Poll<Result<(), Reference>> {
        if self.selections.is_none() {
            self.selections = Some(
                match queries.lower.runtime_staged_queries(queries.weight.scratch) {
                    Ok(selections) => selections,
                    Err(error) => return Poll::Ready(Err(queries.failure_record(error))),
                },
            );
        }
        let selections = self.selections.as_ref().expect("initialized selections");
        while let Some((query, site)) = selections.get(self.index).copied() {
            match cx.read(query) {
                Poll::Pending => {
                    self.waiting = Some(query);
                    return Poll::Pending;
                }
                Poll::Ready(Err(reference)) => {
                    let Some(mut error) = queries.lower.failure(reference) else {
                        return Poll::Ready(Err(reference));
                    };
                    if let Some(site) = site
                        && let QueryError::Diagnostics(diagnostics) = &mut error
                    {
                        for diagnostic in diagnostics {
                            diagnostic.secondary.push(hirsel_diag::Label {
                                span: site,
                                message: "selected size instantiation".into(),
                            });
                        }
                    }
                    return Poll::Ready(Err(queries.failure_record(error)));
                }
                Poll::Ready(Ok(_)) => {
                    self.index += 1;
                }
            }
        }
        self.waiting = None;
        Poll::Ready(Ok(()))
    }
}
