//! The units canary as a command: `units <export> -o <findings>`
//! (`hirsel_analysis_canary::units`).

hirsel_guard::install!();

fn main() -> std::process::ExitCode {
    hirsel_analysis_canary::main("units", hirsel_analysis_canary::units::analyze)
}
