//! Source flow keeps complete diagnostic witnesses and dependency observations.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_types::QueryError;

fn transcript() -> String {
    with_compiler_stack(|| {
        let programs = [
            "fn choose(take x:Bytes,b:bool)->Bytes{if b {take x}else{take x}}",
            "fn moved(take x:Bytes,b:bool)->Bytes{if b {let y=take x};take x}",
            "fn looped(take x:Bytes,b:bool){while b {let y=take x}}",
            "fn repaired(take x:Bytes,b:bool){let mut y=take x;while b {let z=take y;y=b\"new\"}}",
            "fn view(x:Bytes)->Bytes in x{x}\nfn choose(x:Bytes,y:Bytes,b:bool)->Bytes in x{if b{view(x)}else{view(y)}}",
            "fn need(take x:unique Bytes){}\nfn probe(x:Bytes){let a=copy x;need(take a)}",
            "fn need[T:sendable](take x:T){}\nfn probe(){let x:List[u64]=[1u64];need(take x)}",
            "@preserves_captures(it) fn retain[T](mut it:Iter[T]){}\n@advance_only(it) fn step[T](mut it:Iter[T]){retain(mut it)}",
            "@captures(state,next) fn build[S,T](take state:S,take next:fn(mut S)->Option[T])->Iter[T]{iter.from(take state,take next)}",
            "@captures(state) fn build[S,T](take state:S,take next:fn(mut S)->Option[T])->Iter[T]{iter.from(take state,take next)}",
            "fn missing()->u64{absent}\nfn good()->u64{1u64}",
            "fn generic[T](mut x:T,take y:T){x=take y}",
        ];
        let mut text = String::new();
        let mut accepted = 0;
        let mut rejected = 0;
        let mut negative = 0;
        for (index, source) in programs.into_iter().enumerate() {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine
                .sources
                .add("flow-oracle.hir", source.into())
                .unwrap();
            let parsed = engine.parse_file(file, &default_context()).unwrap();
            assert!(
                parsed.diagnostics.is_empty(),
                "{index}: {:?}",
                parsed.diagnostics
            );
            text.push_str(&format!("program {index}\n"));
            for item in &parsed.value.items {
                if !matches!(item.item.kind, hirsel_syntax::ItemKind::Function { .. }) {
                    continue;
                }
                let path = &item.item.definition;
                text.push_str(&format!("{path}\n"));
                let mut observations = Vec::new();
                // A second query exercises memo validation and read replay too.
                for _ in 0..2 {
                    let diagnostics = match engine.mode_definition(path) {
                        Ok(output) => output.diagnostics,
                        Err(QueryError::Diagnostics(diagnostics)) => diagnostics,
                        Err(error) => panic!("{index}: {error:?}"),
                    };
                    if diagnostics
                        .iter()
                        .any(|d| d.severity == hirsel_diag::Severity::Error)
                    {
                        rejected += 1;
                    } else {
                        accepted += 1;
                    }
                    let diagnostics: Vec<_> = diagnostics
                        .iter()
                        .map(|d| d.to_json(&engine.sources))
                        .collect();
                    let mut reads = Vec::new();
                    for kind in [
                        QueryKind::Signature,
                        QueryKind::Resolve,
                        QueryKind::Check,
                        QueryKind::Modes,
                    ] {
                        if let Some(detail) = engine.dependency_reads(path, kind) {
                            negative += detail.lookups.iter().filter(|(_, found)| !found).count();
                            negative += detail.failed_signatures.len();
                        }
                        reads.push(serde_json::json!({
                            "phase": format!("{kind:?}"),
                            "detail": engine.dependency_reads(path, kind),
                            "typed": engine.memo_reads(path, kind),
                        }));
                    }
                    observations.push(serde_json::json!({"diagnostics":diagnostics,"reads":reads}));
                }
                assert_eq!(observations[0], observations[1], "{index}: warm query");
                text.push_str(&observations[0].to_string());
                text.push('\n');
            }
        }
        assert!(accepted > 0 && rejected > 0 && negative > 0);
        text
    })
}

#[test]
fn diagnostics_and_complete_reads_survive_three_flow_salts() {
    let Some(runs) = hirsel_intern::under_salts(
        "modes_flow::diagnostics_and_complete_reads_survive_three_flow_salts",
        &[1, 2, 3],
        transcript,
    ) else {
        return;
    };
    assert_eq!(runs.len(), 3);
    assert!(runs.iter().all(|run| run == &runs[0]));
    // Also retain the complete old/new transcript in focused validation logs.
    eprintln!("FLOW_ORACLE {}", serde_json::to_string(&runs[0]).unwrap());
}
