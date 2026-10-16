//! Time every executable corpus campaign on the same single configuration.
use hirsel_conformance::native;
use hirsel_driver::native::{Engine, Outcome, Toolchain, full_matrix};
fn main() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let matrix: Vec<_> = full_matrix().into_iter().filter(|c| matches!(c.engine,Engine::Heap {campaign:true})
        && c.options.target.triple.starts_with("x86_64")).collect();
    assert_eq!(matrix.len(),1);
    let toolchain = Toolchain::in_tree(&native::scratch("holder-scan-runtime").unwrap(),&[]).unwrap();
    let dir = native::scratch("holder-scan").unwrap();
    let mut slowest = (String::new(),0.0f64);
    let mut measured = 0;
    for case in native::baseline(&root).unwrap() {
        if case.id.contains("self-tail-runtime") || case.id.contains("nested-key-run") {continue}
        let start = std::time::Instant::now();
        let results = native::run_cases(&[case],&matrix,&toolchain,&dir,1);
        let seconds = start.elapsed().as_secs_f64();
        let result = &results[0];
        let agree = result.snapshot.is_none() && result.outcomes.iter().all(|(_,_,o)| *o==Outcome::Agree);
        println!("scan\t{}\t{seconds:.6}\t{agree}",result.id);
        if agree && seconds>slowest.1 {slowest=(result.id.clone(),seconds)}
        measured+=1;
    }
    println!("slowest\t{}\t{:.6}\tmeasured={measured}",slowest.0,slowest.1);
}
