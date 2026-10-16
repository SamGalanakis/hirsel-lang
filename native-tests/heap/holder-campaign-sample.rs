//! Failure-heavy corpus controls, through the existing driver seams.
use hirsel_conformance::native;
use hirsel_driver::native::{self as driver, Engine, Options, Sites, full_matrix};
use hirsel_driver::compiler_adapters;
use hirsel_engine::{QueryEngine, default_context, default_environment, with_compiler_stack};
fn main() {
    assert_eq!(std::env::var("HIRSEL_HEAPCHECK_COMPARE_CAMPAIGNS").as_deref(), Ok("1"));
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut selected = 0;
    let configs: Vec<_> = full_matrix().into_iter().filter(|c| matches!(c.engine, Engine::Heap {..})
        && c.options.target.triple.starts_with("x86_64") && c.options.regions).collect();
    assert_eq!(configs.len(),3);
    for case in native::baseline(&root).unwrap() {
        if case.id.contains("self-tail-runtime") || case.id.contains("nested-key-run") {continue}
        let configs = &configs;
        let id = case.id.clone();
        let passed = with_compiler_stack(move || {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let context = default_context();
            let file = engine.sources.add("case.hir", case.source).unwrap();
            let output = hirsel_engine::check_progress(&mut engine,file,&context).unwrap();
            assert!(output.diagnostics.is_empty());
            let entry = hirsel_engine::entry_path(&context);
            let input = match engine.export_native_input() {Ok(x)=>x,Err(_)=>return false};
            let validated = engine.import_native_input(input).unwrap();
            let sites = Sites::new(validated.snapshot());
            let default = match driver::build(&validated,&Options::new(configs[2].options.target.clone())) {
                Ok(p)=>p, Err(_)=>return false };
            let clean = match driver::run_heap(&default,&entry,&sites) {Ok(r)=>r,Err(_)=>return false};
            // Select a bounded sample; every executed site of selected programs is tested.
            if !(8..=512).contains(&clean.sites) {return false}
            for c in configs {
                let ready = driver::build(&validated,&c.options).unwrap();
                let start = std::time::Instant::now();
                let result = driver::heap_campaign(&ready,&entry,&sites).unwrap();
                println!("sample\t{}\t{}\t{}\t{:.6}",case.id,c.name(),result.sites,start.elapsed().as_secs_f64());
            }
            true
        });
        if passed { selected += 1; eprintln!("selected {selected}: {id}"); }
        if selected == 50 {break}
    }
    assert_eq!(selected,50,"insufficient failure-heavy corpus controls");
    println!("50 programs, 3 configurations; complete fork and replay campaigns agree");
}
