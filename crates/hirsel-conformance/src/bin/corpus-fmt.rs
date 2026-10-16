use std::{env, path::PathBuf, process::ExitCode};
fn main() -> ExitCode {
    hirsel_engine::with_compiler_stack(|| {
        let mut root = env::current_dir().unwrap();
        let mut hirsel = None;
        let mut evidence = None;
        let mut args = env::args_os().skip(1);
        while let Some(arg) = args.next() {
            let target = match arg.to_str() {
                Some("--root") => &mut root,
                Some("--hirsel") => {
                    hirsel = args.next().map(PathBuf::from);
                    continue;
                }
                Some("--evidence") => {
                    evidence = args.next().map(PathBuf::from);
                    continue;
                }
                _ => {
                    eprintln!(
                        "usage: corpus-fmt [--root REPO] --hirsel TARGET/release/hirsel --evidence DIR"
                    );
                    return ExitCode::from(2);
                }
            };
            let Some(value) = args.next() else {
                return ExitCode::from(2);
            };
            *target = PathBuf::from(value);
        }
        let (Some(hirsel), Some(evidence)) = (hirsel, evidence) else {
            eprintln!("--hirsel and --evidence are required");
            return ExitCode::from(2);
        };
        match hirsel_conformance::corpus_format::migrate(&root, &hirsel, &evidence) {
            Ok(result) => {
                println!("{result}");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("{error}");
                ExitCode::FAILURE
            }
        }
    })
}
