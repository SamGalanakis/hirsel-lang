fn cpu_compiler() -> std::process::Command {
    // Preserve tighter launcher budgets while bounding compiler transport fan-out.
    let jobs = std::env::var("HIRSEL_JOBS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|&value| value > 0)
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, usize::from))
        .min(4);
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_hirsel"));
    command.env("HIRSEL_JOBS", jobs.to_string());
    command
}
