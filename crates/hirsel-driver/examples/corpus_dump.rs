//! Dump every root, with optional canonical numbering of equal full type graphs.

hirsel_guard::install!();

#[path = "../tests/support/canonical_tir.rs"]
mod canonical_tir;
use hirsel_driver::*;
use hirsel_engine::*;
fn main() {
    with_compiler_stack(|| {
        let args: Vec<_> = std::env::args().collect();
        let canonical = args.get(2).is_some_and(|s| s == "--canonical");
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine
            .sources
            .add(&args[1], std::fs::read_to_string(&args[1]).unwrap())
            .unwrap();
        match engine.parse_file(file, &default_context()) {
            Err(error) => println!("error\t{error:?}"),
            Ok(parsed) => {
                for diagnostic in &parsed.diagnostics {
                    println!("diagnostic\t{diagnostic:?}");
                }
                for syntax in &parsed.value.items {
                    let path = &syntax.item.definition;
                    match engine.lower_to_tir(path) {
                        Err(error) => println!("error\t{}\t{error:?}", path.0.name),
                        Ok(definition) => {
                            for diagnostic in &definition.diagnostics {
                                println!("diagnostic\t{diagnostic:?}");
                            }
                            let text = if canonical {
                                canonical_tir::definition(&definition.value)
                            } else {
                                hirsel_lower::print::definition(&definition.value)
                            };
                            print!("{text}");
                        }
                    }
                }
            }
        }
    });
}
