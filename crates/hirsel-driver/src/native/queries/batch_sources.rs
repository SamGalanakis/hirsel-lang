use crate::native::receipt_source::{factory::NamedInput, provider::Front};
use hirsel_layout::layout::LayoutBuilder;
use hirsel_rc::instance::{InstanceRole, Instances, Program};

pub(in crate::native) fn capture(
    reader: Option<&hirsel_engine::NativeOriginalReader>,
    program: &Program,
    instances: &mut Instances,
    layouts: &mut LayoutBuilder<'_>,
) -> Result<hirsel_rc::import::SourceData, String> {
    let Some(reader) = reader else {
        return Ok(Default::default());
    };
    let provider = Front::new(reader);
    let roots = instances
        .ids()
        .filter(|id| instances.roles[id.index()] == InstanceRole::Function)
        .map(|id| NamedInput {
            base: instances.lexical_key(id).clone(),
            actuals: instances.actuals[id.index()].clone(),
        })
        .collect();
    let (graph, receipts) = super::receipt_requests::reconstruct(&provider, roots, program)?;
    for request in &graph.requests {
        let candidates = instances
            .ids()
            .filter(|id| {
                instances.lexical_key(*id).definition() == request.ordinary_key().definition()
                    && instances.lexical_key(*id).body() == request.ordinary_key().body()
            })
            .collect::<Vec<_>>();
        let mut selected = None;
        for id in candidates {
            if instances
                .receipt_lexical_matches(id, request, layouts)
                .map_err(|e| format!("batch lexical receipt: {e:?}"))?
            {
                if selected.replace(id).is_some() {
                    return Err("ambiguous batch source receipt".into());
                }
            }
        }
        let id = selected.ok_or("batch source receipt has no lexical implementation")?;
        instances
            .bind_receipt_request(id, request, layouts)
            .map_err(|e| format!("batch source receipt: {e:?}"))?;
    }
    let expectations = super::seal_inventory::current(&provider, &graph, program, |_| true)?;
    Ok(hirsel_rc::import::SourceData {
        expectations,
        receipts,
    })
}
