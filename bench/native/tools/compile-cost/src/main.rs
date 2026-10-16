//! Compile the native runner's exact baseline, without executing its programs.
use hirsel_conformance::native::Case;
use hirsel_driver::native::{OptLevel, Options, Partition, TargetSpec, Toolchain, build, link};
use std::ffi::{c_char, c_void};
use std::path::{Path, PathBuf};
use std::time::Instant;

#[link(name = "dl")]
unsafe extern "C" {
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
}
struct Phases {
    reset: unsafe extern "C" fn(),
    read: unsafe extern "C" fn(u32) -> u64,
}
impl Phases {
    fn load() -> Result<Self, String> {
        // SAFETY: Linux RTLD_DEFAULT resolves the measurement library's exact C
        // signatures. It must be preloaded; absent hooks are an error, not zero.
        unsafe {
            let reset = dlsym(std::ptr::null_mut(), c"hirsel_scalar_perf_reset".as_ptr());
            let read = dlsym(std::ptr::null_mut(), c"hirsel_scalar_perf_read".as_ptr());
            if reset.is_null() || read.is_null() {
                return Err("preload the compile-cost phases library".into());
            }
            Ok(Self {
                reset: std::mem::transmute::<*mut c_void, unsafe extern "C" fn()>(reset),
                read: std::mem::transmute::<*mut c_void, unsafe extern "C" fn(u32) -> u64>(read),
            })
        }
    }
}
fn compile(case: &Case, target: &TargetSpec, tools: &Toolchain, dir: &Path) -> Result<(), String> {
    let path = case
        .path
        .as_deref()
        .ok_or("a baseline case without a source path")?;
    let mut program =
        hirsel_driver::load_program_source(path, Some(&case.source)).map_err(|e| e.to_string())?;
    // This is the native runner's header-only entry selection. Package sidecars
    // select their own entry; no alternate checking or lowering implementation.
    if !path.with_extension("hir.json").exists() {
        for line in case
            .source
            .lines()
            .take_while(|l| l.trim().is_empty() || l.trim_start().starts_with("//"))
        {
            if let Some(entry) = line.trim_start().strip_prefix("// entry:") {
                program.entry.0.name = entry.trim().to_owned();
            }
        }
    }
    let checked = program.check().map_err(|e| format!("{e:?}"))?;
    if let Some(diagnostic) = checked.diagnostics.first() {
        return Err(format!("{} {}", diagnostic.rule, diagnostic.message));
    }
    let input = program
        .engine
        .export_native_input()
        .map_err(|e| format!("export: {e:?}"))?;
    let validated = program
        .engine
        .import_native_input(input)
        .map_err(|e| format!("import: {e:?}"))?;
    let ready = build(&validated, &Options::new(target.clone())).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    link(
        &ready,
        &program.entry,
        OptLevel::Speed,
        Partition::Whole,
        tools,
        dir,
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if !(2..=3).contains(&args.len()) {
        return Err("usage: hirsel-compile-cost ROOT OUTPUT [CASE_FILTER]".into());
    }
    let phases = Phases::load()?;
    // No compiler worker has started yet. The current process keeps the loaded
    // interposer, while runtime-build/link children receive a clean environment.
    unsafe {
        std::env::remove_var("LD_PRELOAD");
    }
    let root = PathBuf::from(&args[0]);
    let output = PathBuf::from(&args[1]);
    let level = std::env::var("HIRSEL_PERF_LEVEL").map_err(|e| e.to_string())?;
    let mut cases = hirsel_conformance::native::baseline(&root)?;
    let baseline = cases.len();
    if let Some(filter) = args.get(2) {
        cases.retain(|c| c.id.contains(filter));
    }
    if cases.is_empty() {
        return Err("no baseline case matched".into());
    }
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let runtime_start = Instant::now();
    let tools =
        Toolchain::in_tree(&output.join("runtime"), &[&target]).map_err(|e| e.to_string())?;
    println!(
        "{}",
        serde_json::json!({"kind":"context", "level":level,"baseline":baseline,"selected":cases.len(),"runtime_build_seconds_excluded":runtime_start.elapsed().as_secs_f64(),"runtime_archive":tools.archive(&target.triple),"configuration":"default production, whole partition, x86-64"})
    );
    hirsel_engine::with_compiler_stack(|| {
        for (index, case) in cases.iter().enumerate() {
            // SAFETY: the hooks' counters belong only to this measurement process.
            unsafe {
                (phases.reset)();
            }
            let start = Instant::now();
            let result = compile(case, &target, &tools, &output.join(format!("case-{index}")));
            let total = start.elapsed().as_secs_f64();
            let counters: [u64; 4] = std::array::from_fn(|i| unsafe { (phases.read)(i as u32) });
            let optimizer = counters[0] as f64 / 1e9;
            let emission = counters[1] as f64 / 1e9;
            assert!(
                result.is_err() || (counters[2] > 0 && counters[3] > 0),
                "LLVM interposition did not observe {}",
                case.id
            );
            println!(
                "{}",
                serde_json::json!({"kind":"case","id":case.id,"level":level,"end_to_end_seconds":total,"optimization_seconds":optimizer,"emission_seconds":emission,"optimization_emission_seconds":optimizer+emission,"optimization_calls":counters[2],"emission_calls":counters[3],"error":result.err()})
            );
        }
    });
    Ok(())
}
fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
