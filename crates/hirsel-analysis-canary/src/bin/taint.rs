//! The taint canary as a command: `taint <export> -o <findings>`
//! (`hirsel_analysis_canary::taint`).

hirsel_guard::install!();

fn main() -> std::process::ExitCode {
    hirsel_analysis_canary::main("taint", hirsel_analysis_canary::taint::analyze)
}
