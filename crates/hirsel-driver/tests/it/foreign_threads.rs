//! S-13 extern dispatch (design §6 Extern dispatch, lane R8): each `unsafe ffi` package's
//! trusted `threads`, the precedence rule that picks the thread an extern call runs on, the
//! `serial` gate, callback permits and the build's nesting check, on native executables
//! whose C code reports the thread ids it ran on. Each program prints one digit per check.
use hirsel_driver::*;
use hirsel_engine::*;
use serde_json::json;
use std::path::{Path, PathBuf};

/// Thread ids, callback drivers and a gate witness, shared by every package's C file: each
/// package's symbols carry its prefix.
fn c_library(prefix: &str) -> String {
    format!(
        r#"#include <stdint.h>
#include <sys/syscall.h>
#include <unistd.h>

static uint64_t thread_id(void) {{ return (uint64_t)syscall(SYS_gettid); }}

uint64_t {prefix}_tid(void) {{ return thread_id(); }}
uint64_t {prefix}_tid_blocking(void) {{ return thread_id(); }}

/* Calls f(ctx, its own thread id) n times; the result is the last callback's. */
uint64_t {prefix}_call(void *ctx, uint64_t (*f)(void *, uint64_t), uint64_t n) {{
    uint64_t last = 0;
    for (uint64_t i = 0; i < n; i++) last = f(ctx, thread_id());
    return last;
}}
uint64_t {prefix}_call_blocking(void *ctx, uint64_t (*f)(void *, uint64_t), uint64_t n) {{
    return {prefix}_call(ctx, f, n);
}}

/* The gate witness: a call chain is the calls between an outermost entry and its return;
   a call from another chain while one is inside is an overlap. */
static uint64_t depth, owner, overlaps, deepest, entries;
uint64_t {prefix}_chain(void *ctx, uint64_t (*f)(void *, uint64_t), uint64_t tag) {{
    if (depth == 0) {{ owner = tag; entries = entries * 10 + tag; }}
    else if (owner != tag) overlaps++;
    depth++;
    if (depth > deepest) deepest = depth;
    uint64_t result = f(ctx, tag);
    depth--;
    return result;
}}
uint64_t {prefix}_overlaps(void) {{ return overlaps; }}
uint64_t {prefix}_deepest(void) {{ return deepest; }}
uint64_t {prefix}_entries(void) {{ return entries; }}
uint64_t {prefix}_visit(void *ctx, uint64_t (*f)(void *, uint64_t), uint64_t n) {{
    return {prefix}_call(ctx, f, n);
}}
uint64_t {prefix}_visit_blocking(void *ctx, uint64_t (*f)(void *, uint64_t), uint64_t n) {{
    return {prefix}_call(ctx, f, n);
}}
"#
    )
}

/// The Hirsel side of a package whose C symbols carry `prefix`: its externs, safe wrappers
/// and callbacks that reach only this package.
fn hirsel_library(prefix: &str) -> String {
    format!(
        r#"pub type Probe {{
  u: Unsafe,
  seen: u64,
  nested: u64,
  inline: u64,
  items: List[u64],
}}

/// A call chain's state: how deep it is and how its innermost callback waits (0 not at
/// all, 1 for a child task, 2 for a child task that calls this package).
pub type Chain {{
  u: Unsafe,
  tasks: Tasks,
  depth: u64,
  wait: u64,
}}

/// A suspending callback's state: its thread before and after it waited.
pub type Visit {{
  u: Unsafe,
  tasks: Tasks,
  first: u64,
  then: u64,
}}

/// Returns the calling thread's id and retains nothing.
extern "c" fn {prefix}_tid(u: Unsafe) -> u64

/// Returns the calling thread's id and retains nothing.
@blocking extern "c" fn {prefix}_tid_blocking(u: Unsafe) -> u64

/// Calls f(ctx, its thread id) n times during the call and retains neither.
extern "c" fn {prefix}_call(
  u: Unsafe,
  mut ctx: unique Probe,
  f: FnPtr[fn(mut state: unique Probe, id: u64) -> u64],
  n: u64
) -> u64

/// Calls f(ctx, its thread id) n times during the call and retains neither.
@blocking extern "c" fn {prefix}_call_blocking(
  u: Unsafe,
  mut ctx: unique Probe,
  f: FnPtr[fn(mut state: unique Probe, id: u64) -> u64],
  n: u64
) -> u64

/// Calls f(ctx, tag) once inside the call chain tag and retains neither.
extern "c" fn {prefix}_chain(
  u: Unsafe,
  mut ctx: unique Chain,
  f: FnPtr[fn(mut state: unique Chain, tag: u64) -> u64],
  tag: u64
) -> u64

/// Calls f(ctx, its thread id) once during the call and retains neither.
extern "c" fn {prefix}_visit(
  u: Unsafe,
  mut ctx: unique Visit,
  f: FnPtr[fn(mut state: unique Visit, id: u64) -> u64],
  n: u64
) -> u64

/// Calls f(ctx, its thread id) once during the call and retains neither.
@blocking extern "c" fn {prefix}_visit_blocking(
  u: Unsafe,
  mut ctx: unique Visit,
  f: FnPtr[fn(mut state: unique Visit, id: u64) -> u64],
  n: u64
) -> u64

/// Reads the gate witness; retains nothing.
extern "c" fn {prefix}_overlaps(u: Unsafe) -> u64

/// Reads the gate witness; retains nothing.
extern "c" fn {prefix}_deepest(u: Unsafe) -> u64

/// Reads the gate witness; retains nothing.
extern "c" fn {prefix}_entries(u: Unsafe) -> u64

pub fn tid(u: Unsafe) -> u64 {{
  {prefix}_tid(u)
}}

pub fn tid_blocking(u: Unsafe) -> u64 {{
  {prefix}_tid_blocking(u)
}}

/// A callback: records the interval's thread, a nested blocking call's and a nested
/// non-blocking call's, and allocates on the thread it runs on.
fn record(mut p: Probe, id: u64) -> u64 {{
  p.seen = id
  p.nested = {prefix}_tid_blocking(p.u)
  p.inline = {prefix}_tid(p.u)
  for i in 0u64..64u64 {{
    list.push(mut p.items, i)
  }}
  id
}}

/// The thread ids a call's callback saw, n times: (interval, nested blocking, nested
/// non-blocking, how many items its callbacks pushed).
pub fn probed(u: Unsafe, blocking: bool, n: u64) -> (u64, u64, u64, u64) {{
  let mut p = Probe(u: u, seen: 0u64, nested: 0u64, inline: 0u64, items: [])
  if blocking {{
    let _ = {prefix}_call_blocking(u, mut p, record, n)
  }} else {{
    let _ = {prefix}_call(u, mut p, record, n)
  }}
  (p.seen, p.nested, p.inline, list.len(p.items))
}}

/// The running task waits for a child task: one that calls this package when `calls`.
fn pause(u: Unsafe, tasks: Tasks, calls: bool) -> u64 {{
  task.scope(tasks, fn(s: Scope) -> u64 {{
    let child = task.async(s, fn() -> u64 {{
      if calls {{
        {prefix}_tid(u)
      }} else {{
        0u64
      }}
    }})
    task.await(s, take child)
  }})
}}

fn nest(mut c: Chain, tag: u64) -> u64 {{
  if c.depth < 3u64 {{
    c.depth += 1u64
    let u = c.u
    {prefix}_chain(u, mut c, nest, tag)
  }} else {{
    if c.wait > 0u64 {{
      let _ = pause(c.u, c.tasks, c.wait == 2u64)
    }}
    tag
  }}
}}

/// A call chain nested four deep through callbacks into this package, whose innermost
/// callback waits as `wait` says.
pub fn nested_chain(u: Unsafe, tasks: Tasks, tag: u64, wait: u64) -> u64 {{
  let mut c = Chain(u: u, tasks: tasks, depth: 0u64, wait: wait)
  {prefix}_chain(u, mut c, nest, tag)
}}

fn linger(mut v: Visit, id: u64) -> u64 {{
  v.first = id
  let _ = pause(v.u, v.tasks, false)
  v.then = {prefix}_tid(v.u)
  id
}}

/// A callback of this package's call (its blocking one when `blocking`) that waits for a
/// child task: its thread before and after the wait.
pub fn visit(u: Unsafe, tasks: Tasks, blocking: bool) -> (u64, u64) {{
  let mut v = Visit(u: u, tasks: tasks, first: 0u64, then: 0u64)
  if blocking {{
    let _ = {prefix}_visit_blocking(u, mut v, linger, 1u64)
  }} else {{
    let _ = {prefix}_visit(u, mut v, linger, 1u64)
  }}
  (v.first, v.then)
}}

/// Hands f to this package's call (its blocking extern when `blocking`) once: the thread id
/// it passed.
pub fn call_with(
  u: Unsafe,
  mut p: unique Probe,
  f: FnPtr[fn(mut state: unique Probe, id: u64) -> u64],
  blocking: bool
) -> u64 {{
  if blocking {{
    {prefix}_call_blocking(u, mut p, f, 1u64)
  }} else {{
    {prefix}_call(u, mut p, f, 1u64)
  }}
}}

/// (overlapping calls seen, deepest nesting, outermost entries in order as decimal digits)
pub fn witness(u: Unsafe) -> (u64, u64, u64) {{
  ({prefix}_overlaps(u), {prefix}_deepest(u), {prefix}_entries(u))
}}
"#
    )
}

/// The wrappers of a package's library: every safe function, none with a model.
fn wrappers() -> serde_json::Value {
    json!(
        [
            "tid",
            "tid_blocking",
            "probed",
            "nested_chain",
            "call_with",
            "visit",
            "witness",
        ]
        .iter()
        .map(|name| json!({"module":[],"name":name,"kind":"effect","model":"none"}))
        .collect::<Vec<_>>()
    )
}

/// A launch selecting the packages `homelib` (default `home`), `anylib` and `serlib` with
/// `main`.
struct Fixture {
    root: PathBuf,
    manifest: PathBuf,
}

fn fixture(name: &str, main: &str) -> Fixture {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("foreign-threads-{name}"));
    let _ = std::fs::remove_dir_all(&root);
    let target = native::TargetSpec::x86_64_unknown_linux_gnu();
    let tools = native::Toolchain::in_tree(&root.join("runtime"), &[&target]).unwrap();
    let mut selected = Vec::new();
    for (package, threads) in [
        ("homelib", None),
        ("anylib", Some("any")),
        ("serlib", Some("serial")),
    ] {
        let dir = root.join(package);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("lib.c"), c_library(package)).unwrap();
        let compiled = std::process::Command::new(&tools.clang)
            .args(["-c", "-O1", "-fPIC"])
            .arg(dir.join("lib.c"))
            .arg("-o")
            .arg(dir.join("lib.o"))
            .output()
            .unwrap();
        assert!(compiled.status.success(), "{compiled:?}");
        std::fs::write(dir.join("lib.hir"), hirsel_library(package)).unwrap();
        let mut foreign = json!({"link":["lib.o"],"headers_checked":false,"wrappers":wrappers()});
        if let Some(threads) = threads {
            foreign["threads"] = json!(threads);
        }
        let manifest = json!({"schema":1,"package":{"name":package,"version":"1"},
            "sources":[{"path":"lib.hir","module":[]}],"dependencies":[],
            "unsafe_ffi":foreign});
        std::fs::write(dir.join("hirsel.json"), manifest.to_string()).unwrap();
        selected.push(json!({"manifest":format!("{package}/hirsel.json"),
            "package":{"name":package,"version":"1"},
            "expose":[{"head":package,"module":[]}]}));
    }
    std::fs::write(root.join("main.hir"), main).unwrap();
    std::fs::write(
        root.join("grants.json"),
        r#"{"io":{"stdout":"stdout.bytes@1","tasks":"tasks.cooperative@1","dirs":[]},"unsafe":true}"#,
    )
    .unwrap();
    let manifest = json!({"schema":1,"package":{"name":"app","version":"1"},
        "entry":{"module":[],"name":"main"},"sources":[{"path":"main.hir","module":[]}],
        "dependencies":selected});
    let path = root.join("hirsel.json");
    std::fs::write(&path, manifest.to_string()).unwrap();
    Fixture {
        root,
        manifest: path,
    }
}

impl Fixture {
    /// Build the executable with `backend`: the build's verdict, with the check's
    /// diagnostics when the program does not check.
    fn build(&self, backend: native::Backend) -> Result<PathBuf, String> {
        let executable = self.root.join(format!("app-{backend:?}"));
        let built = native::build_path_native_backend(
            &self.manifest,
            &executable,
            native::TargetSpec::x86_64_unknown_linux_gnu(),
            native::BuildProfile::Default,
            backend,
            None,
            false,
            hirsel_engine::Verify::On,
        )
        .map_err(|error| {
            let diagnostics = check_path(&self.manifest)
                .map(|output| {
                    output
                        .diagnostics
                        .iter()
                        .map(|d| d.render_human(&output.sources))
                        .collect::<String>()
                })
                .unwrap_or_default();
            format!("{error}\n{diagnostics}")
        })?;
        built.map_err(|error| format!("{error:?}"))?;
        Ok(executable)
    }
    fn run(&self, executable: &Path) -> std::process::Output {
        std::process::Command::new(executable)
            .arg("--hirsel-grants")
            .arg(self.root.join("grants.json"))
            .output()
            .unwrap()
    }
    /// Build and run on both backends; each run's stdout.
    fn outputs(&self) -> Vec<String> {
        [native::Backend::Llvm, native::Backend::Cranelift]
            .into_iter()
            .map(|backend| {
                let executable = self
                    .build(backend)
                    .unwrap_or_else(|e| panic!("{backend:?}: {e}"));
                let output = self.run(&executable);
                assert!(output.status.success(), "{backend:?}: {output:?}");
                String::from_utf8(output.stdout).unwrap()
            })
            .collect()
    }
}

/// The done-when's thread-id assertion: `any` and `serial` non-blocking externs run on the
/// caller's worker (home until lane W1); a default-`home` `@blocking` call runs on home,
/// never on the pool; `any` and `serial` `@blocking` calls run on a blocking-pool thread.
#[test]
fn externs_run_on_the_thread_their_package_and_blocking_select() {
    with_compiler_stack(|| {
        let main = r#"pub type AppError = Grant(GrantError) | Output(OutputError)

pub fn main(rt: Io, u: Unsafe) -> Result[(), AppError] {
  let home = homelib.tid(u)
  let stdout = io.stdout(rt) ? Grant
  out.print(stdout, if homelib.tid_blocking(u) == home { "1" } else { "0" }) ? Output
  out.print(stdout, if anylib.tid(u) == home { "1" } else { "0" }) ? Output
  out.print(stdout, if serlib.tid(u) == home { "1" } else { "0" }) ? Output
  out.print(stdout, if serlib.tid_blocking(u) != home { "1" } else { "0" }) ? Output
  out.print(stdout, if anylib.tid_blocking(u) != home { "1" } else { "0" }) ? Output
  out.print(stdout, "\n") ? Output
  Ok(())
}
"#;
        for output in fixture("tids", main).outputs() {
            assert_eq!(output, "11111\n");
        }
    });
}

/// Callbacks run on their interval's thread and allocate there (a pool thread's own heap
/// for a blocking `any` or `serial` call), one at a time with the run's one compute
/// permit; a nested call, blocking or not, stays on the interval's thread; a `home`
/// blocking call and its callbacks stay on home; a nested same-package chain under
/// `serial` re-enters its gate (deepest 4, one outermost entry).
#[test]
fn callbacks_run_and_allocate_on_their_interval_thread() {
    with_compiler_stack(|| {
        let main = r#"pub type AppError = Grant(GrantError) | Output(OutputError)

pub fn main(rt: Io, u: Unsafe) -> Result[(), AppError] {
  let tasks = io.tasks(rt) ? Grant
  let home = homelib.tid(u)
  let stdout = io.stdout(rt) ? Grant
  let (s1, n1, i1, c1) = anylib.probed(u, true, 3u64)
  let ok1 = s1 != home && n1 == s1 && i1 == s1 && c1 == 192u64
  out.print(stdout, if ok1 { "1" } else { "0" }) ? Output
  let (s2, n2, i2, c2) = anylib.probed(u, false, 2u64)
  let ok2 = s2 == home && n2 == s2 && i2 == s2 && c2 == 128u64
  out.print(stdout, if ok2 { "1" } else { "0" }) ? Output
  let (s3, n3, i3, c3) = homelib.probed(u, true, 2u64)
  let ok3 = s3 == home && n3 == s3 && i3 == s3 && c3 == 128u64
  out.print(stdout, if ok3 { "1" } else { "0" }) ? Output
  let (s4, n4, i4, c4) = serlib.probed(u, true, 2u64)
  let ok4 = s4 != home && n4 == s4 && i4 == s4 && c4 == 128u64
  out.print(stdout, if ok4 { "1" } else { "0" }) ? Output
  let tag = serlib.nested_chain(u, tasks, 7u64, 0u64)
  let (overlaps, deepest, entries) = serlib.witness(u)
  let ok5 = tag == 7u64 && overlaps == 0u64 && deepest == 4u64 && entries == 7u64
  out.print(stdout, if ok5 { "1" } else { "0" }) ? Output
  out.print(stdout, "\n") ? Output
  Ok(())
}
"#;
        for output in fixture("intervals", main).outputs() {
            assert_eq!(output, "11111\n");
        }
    });
}

/// A callback that suspends inside its extern call (it awaits a child task, which runs on
/// home) resumes on the interval's thread: home under a `home` package, its pool thread
/// under an `any` or `serial` blocking call, home under an `any` non-blocking one.
#[test]
fn a_callback_suspends_and_resumes_on_its_interval_thread() {
    with_compiler_stack(|| {
        let main = r#"pub type AppError = Grant(GrantError) | Output(OutputError)

pub fn main(rt: Io, u: Unsafe) -> Result[(), AppError] {
  let tasks = io.tasks(rt) ? Grant
  let home = homelib.tid(u)
  let stdout = io.stdout(rt) ? Grant
  let (b1, a1) = homelib.visit(u, tasks, true)
  out.print(stdout, if b1 == home && a1 == home { "1" } else { "0" }) ? Output
  let (b2, a2) = serlib.visit(u, tasks, true)
  out.print(stdout, if b2 != home && a2 == b2 { "1" } else { "0" }) ? Output
  let (b3, a3) = anylib.visit(u, tasks, true)
  out.print(stdout, if b3 != home && a3 == b3 { "1" } else { "0" }) ? Output
  let (b4, a4) = anylib.visit(u, tasks, false)
  out.print(stdout, if b4 == home && a4 == home { "1" } else { "0" }) ? Output
  out.print(stdout, "\n") ? Output
  Ok(())
}
"#;
        for output in fixture("suspend", main).outputs() {
            assert_eq!(output, "1111\n");
        }
    });
}

/// The `serial` gate orders whole call chains: two tasks each run a four-deep chain into
/// `serlib` whose innermost callback suspends; the second waits for the gate until the
/// first chain returns, so the C side sees no overlap and the chains in spawn order. The
/// same program on `anylib` (no gate) does overlap, which shows the tasks interleaved.
#[test]
fn the_serial_gate_orders_whole_call_chains() {
    with_compiler_stack(|| {
        let main = r#"pub type AppError = Grant(GrantError) | Output(OutputError)

pub fn main(rt: Io, u: Unsafe) -> Result[(), AppError] {
  let tasks = io.tasks(rt) ? Grant
  let stdout = io.stdout(rt) ? Grant
  task.scope(tasks, fn(s: Scope) {
    let a = task.async(s, fn() -> u64 { serlib.nested_chain(u, tasks, 1u64, 1u64) })
    let b = task.async(s, fn() -> u64 { serlib.nested_chain(u, tasks, 2u64, 1u64) })
    let c = task.async(s, fn() -> u64 { anylib.nested_chain(u, tasks, 1u64, 1u64) })
    let d = task.async(s, fn() -> u64 { anylib.nested_chain(u, tasks, 2u64, 1u64) })
    let _ = task.await(s, take a) + task.await(s, take b)
    let _ = task.await(s, take c) + task.await(s, take d)
  })
  let (o1, d1, e1) = serlib.witness(u)
  out.print(stdout, if o1 == 0u64 && d1 == 4u64 && e1 == 12u64 { "1" } else { "0" }) ? Output
  let (o2, d2, _) = anylib.witness(u)
  out.print(stdout, if o2 > 0u64 && d2 > 4u64 { "1" } else { "0" }) ? Output
  out.print(stdout, "\n") ? Output
  Ok(())
}
"#;
        for output in fixture("gate-order", main).outputs() {
            assert_eq!(output, "11\n");
        }
    });
}

/// A chain that holds `serlib`'s gate while it awaits a child task that calls `serlib`:
/// the child waits for the gate (`foreign.gate`, a CONC-8 wait), its parent for the
/// child, and the run is Blocked.
#[test]
fn a_gate_held_across_a_wait_for_its_own_package_blocks_the_run() {
    with_compiler_stack(|| {
        let main = r#"pub fn main(rt: Io, u: Unsafe) -> Result[(), GrantError] {
  let tasks = io.tasks(rt)?
  let _ = serlib.nested_chain(u, tasks, 1u64, 2u64)
  Ok(())
}
"#;
        let fixture = fixture("gate-blocked", main);
        for backend in [native::Backend::Llvm, native::Backend::Cranelift] {
            let output = fixture.run(&fixture.build(backend).unwrap_or_else(|e| panic!("{e}")));
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(!output.status.success(), "{backend:?}: {output:?}");
            assert!(stderr.contains("foreign.gate"), "{backend:?}: {output:?}");
        }
    });
}

/// A `home` package's blocking extern runs on home, so a callback handed to it may reach a
/// `home` package: the build accepts it and every call runs on home.
#[test]
fn a_home_blocking_extern_may_take_a_callback_that_reaches_home() {
    with_compiler_stack(|| {
        let main = r#"pub type AppError = Grant(GrantError) | Output(OutputError)

fn on_home(mut p: homelib.Probe, id: u64) -> u64 {
  p.seen = id
  homelib.tid(p.u)
}

pub fn main(rt: Io, u: Unsafe) -> Result[(), AppError] {
  let home = homelib.tid(u)
  let mut p = homelib.Probe(u: u, seen: 0u64, nested: 0u64, inline: 0u64, items: [])
  let nested = homelib.call_with(u, mut p, on_home, true)
  let ok = p.seen == home && nested == home
  out.print(io.stdout(rt) ? Grant, if ok { "1\n" } else { "0\n" }) ? Output
  Ok(())
}
"#;
        for output in fixture("home-callback", main).outputs() {
            assert_eq!(output, "1\n");
        }
    });
}

/// S-13's nesting check: a callback that can reach a `home` package's extern, handed to an
/// `any` package's extern or to a `serial` package's blocking one, refuses the build,
/// naming the callback, the home extern it reaches and the extern it is handed to.
#[test]
fn a_callback_reaching_home_handed_to_another_package_refuses_the_build() {
    with_compiler_stack(|| {
        for (threads, package, blocking) in
            [("any", "anylib", "false"), ("serial", "serlib", "true")]
        {
            let main = format!(
                r#"fn sneak(mut p: {package}.Probe, id: u64) -> u64 {{
  homelib.tid(p.u)
}}

pub fn main(u: Unsafe) -> Result[(), ()] {{
  let mut p = {package}.Probe(u: u, seen: 0u64, nested: 0u64, inline: 0u64, items: [])
  let _ = {package}.call_with(u, mut p, sneak, {blocking})
  Ok(())
}}
"#
            );
            let fixture = fixture(&format!("refuse-{threads}"), &main);
            for backend in [native::Backend::Llvm, native::Backend::Cranelift] {
                let error = fixture.build(backend).unwrap_err();
                assert!(error.contains("PROG-4"), "{error}");
                assert!(error.contains("sneak"), "{error}");
                assert!(error.contains("homelib_tid"), "{error}");
                assert!(error.contains(&format!("{package}_call")), "{error}");
                assert!(error.contains(&format!("the {threads} package")), "{error}");
            }
        }
    });
}

/// `threads` is a trusted manifest member: `home` by default, `serial` or `any`, reported
/// by check (an unknown value is in `foreign::unsafe_ffi_metadata_errors_are_tool_errors`).
#[test]
fn threads_is_a_trusted_manifest_member() {
    with_compiler_stack(|| {
        let fixture = fixture(
            "manifest",
            "pub fn main(u: Unsafe) -> Result[(), ()] {\n  Ok(())\n}\n",
        );
        let output = check_path(&fixture.manifest).unwrap();
        let reported: Vec<_> = output
            .unsafe_ffi
            .iter()
            .map(|p| (p.package.name.as_str(), p.threads.name()))
            .collect();
        assert_eq!(
            reported,
            [("anylib", "any"), ("homelib", "home"), ("serlib", "serial")]
        );
    });
}
