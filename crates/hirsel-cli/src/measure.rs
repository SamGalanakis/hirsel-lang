//! `--timings[=json]` and `--mem-usage` (docs/building.md, "Compile timings").
use hirsel_intern::{memory, timings};
use std::ffi::OsString;
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Format {
    Table,
    Json,
}

/// What the command measures; both reports go to stderr after it finishes.
#[derive(Debug, Default)]
pub struct Measure {
    timings: Option<Format>,
    memory: bool,
}

/// The commands that compile: check, build, run and test.
const COMMANDS: [&str; 4] = ["check", "build", "run", "test"];

impl Measure {
    /// Take `--timings`, `--timings=json` and `--mem-usage` from anywhere
    /// before a `--` separator, as `--verbose` is taken.
    pub fn take(args: &mut Vec<OsString>) -> Result<Self, String> {
        let end = args.iter().position(|a| a == "--").unwrap_or(args.len());
        let mut measure = Measure::default();
        let mut index = 0;
        let mut error = None;
        args.retain(|arg| {
            index += 1;
            if index > end {
                return true;
            }
            match arg.to_str() {
                Some("--timings") => measure.timings = Some(Format::Table),
                Some("--timings=json") => measure.timings = Some(Format::Json),
                Some("--mem-usage") => measure.memory = true,
                Some(other) if other.starts_with("--timings=") => {
                    error = Some(format!(
                        "invalid option '{other}': expected --timings or --timings=json"
                    ));
                }
                _ => return true,
            }
            false
        });
        if let Some(error) = error {
            return Err(error);
        }
        let measuring = measure.timings.is_some() || measure.memory;
        let command = args.first().and_then(|a| a.to_str());
        if measuring && !command.is_some_and(|c| COMMANDS.contains(&c)) {
            return Err("--timings and --mem-usage apply to check, build, run and test".into());
        }
        Ok(measure)
    }

    /// Start recording before the command runs.
    pub fn start(&self) {
        if self.timings.is_some() {
            timings::enable();
        }
        if self.memory {
            memory::install(crate::allocator::count_live());
        }
    }

    /// Print the reports for `command`, which took `wall`.
    pub fn finish(&self, command: &str, wall: Duration) {
        if self.timings.is_none() && !self.memory {
            return;
        }
        let memory = self.memory.then(|| {
            hirsel_engine::release_shared_caches();
            Memory {
                peak_rss: peak_rss(),
                held: memory::records(),
            }
        });
        let report = self.timings.map(|_| timings::report());
        let text = match (self.timings, report) {
            (Some(Format::Json), Some(report)) => {
                json(command, wall, &report, memory.as_ref()).to_string() + "\n"
            }
            (_, report) => {
                let mut text = String::new();
                if let Some(report) = report {
                    text += &table(command, wall, &report);
                }
                if let Some(memory) = &memory {
                    text += &memory.table();
                }
                text
            }
        };
        eprint!("{text}");
    }
}

struct Memory {
    peak_rss: Option<u64>,
    held: Vec<(&'static str, i64)>,
}

impl Memory {
    fn table(&self) -> String {
        let mib = |bytes: i64| format!("{:>10.1} MiB", bytes as f64 / (1 << 20) as f64);
        let mut text = String::from("hirsel mem-usage\n");
        match self.peak_rss {
            Some(bytes) => text += &format!("{:<32}{}\n", "peak RSS", mib(bytes as i64)),
            None => text += &format!("{:<32}{:>14}\n", "peak RSS", "unknown"),
        }
        text += "held at exit, largest first\n";
        // Rows under 64 KiB appear only in the JSON form.
        for (name, bytes) in self.held.iter().filter(|(_, b)| b.abs() >= 64 << 10) {
            text += &format!("  {:<30}{}\n", name, mib(*bytes));
        }
        text
    }
}

/// The process's peak resident set (`VmHWM`), where the OS reports it.
fn peak_rss() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kib * 1024)
}

fn milliseconds(duration: Duration) -> String {
    format!("{:>10.1}", duration.as_secs_f64() * 1e3)
}

fn share(part: Duration, wall: Duration) -> String {
    let percent = if wall.is_zero() {
        0.0
    } else {
        part.as_secs_f64() / wall.as_secs_f64() * 100.0
    };
    format!("{percent:>7.1}%")
}

/// The wall time no interval covers.
fn untimed(wall: Duration, report: &timings::Report) -> Duration {
    wall.saturating_sub(report.intervals.iter().map(|(_, w)| *w).sum())
}

fn table(command: &str, wall: Duration, report: &timings::Report) -> String {
    let mut text = format!(
        "hirsel timings: {command}, wall {:.1} ms\n",
        wall.as_secs_f64() * 1e3
    );
    text += &format!("{:<20}{:>10} {:>8}\n", "interval", "wall ms", "wall");
    let interval = |name: &str, time: Duration| {
        format!("{name:<20}{} {}\n", milliseconds(time), share(time, wall))
    };
    for (i, time) in &report.intervals {
        text += &interval(i.name(), *time);
    }
    text += &interval("(untimed)", untimed(wall, report));
    text += &format!(
        "{:<20}{:>10} {:>10} {:>9}\n",
        "phase", "work ms", "cpu ms", "calls"
    );
    for p in &report.phases {
        text += &format!(
            "{:<20}{} {} {:>9}\n",
            p.phase.name(),
            milliseconds(p.work),
            milliseconds(p.cpu),
            p.calls
        );
    }
    let split = &report.comptime_verify;
    text += &format!(
        "{:<20}{} {} {:>9}\n",
        "  verify in comptime",
        milliseconds(split.work),
        milliseconds(split.cpu),
        split.calls
    );
    text += &format!("{:<20}{}\n", "wait", milliseconds(report.wait));
    text += "count\n";
    for (count, value) in &report.counts {
        text += &format!("  {:<18}{value:>10}\n", count.name());
    }
    let runner = hirsel_driver::native::counts();
    text += &format!(
        "runner\n  {:<18}{:>10}\n  {:<18}{:>10}\n",
        "image runs", runner.images, "cached images", runner.cached
    );
    for (kind, count) in &runner.fallbacks {
        text += &format!("  fallback: {} ({count})\n", runner.reasons[kind]);
    }
    text
}

fn json(
    command: &str,
    wall: Duration,
    report: &timings::Report,
    memory: Option<&Memory>,
) -> serde_json::Value {
    let nanos = |d: Duration| d.as_nanos() as u64;
    let mut value = serde_json::json!({
        "schema": 1,
        "command": command,
        "wall_ns": nanos(wall),
        "untimed_ns": nanos(untimed(wall, report)),
        "intervals": report.intervals.iter().map(|(i, time)| serde_json::json!({
            "interval": i.name(),
            "wall_ns": nanos(*time),
        })).collect::<Vec<_>>(),
        "phases": report.phases.iter().map(|p| serde_json::json!({
            "phase": p.phase.name(),
            "work_ns": nanos(p.work),
            "cpu_ns": nanos(p.cpu),
            "calls": p.calls,
        })).collect::<Vec<_>>(),
        "comptime_verify": {
            "work_ns": nanos(report.comptime_verify.work),
            "cpu_ns": nanos(report.comptime_verify.cpu),
            "calls": report.comptime_verify.calls,
        },
        "wait_ns": nanos(report.wait),
        "counts": report.counts
            .iter()
            .map(|(count, value)| (count.name().to_string(), (*value).into()))
            .collect::<serde_json::Map<_, _>>(),
    });
    let runner = hirsel_driver::native::counts();
    value["runner"] = serde_json::json!({
        "image_runs": runner.images,
        "cached_images": runner.cached,
        "fallbacks": runner.fallbacks.iter().map(|(kind, count)| serde_json::json!({
            "reason": runner.reasons[kind],
            "count": count,
        })).collect::<Vec<_>>(),
    });
    if let Some(memory) = memory {
        value["memory"] = serde_json::json!({
            "peak_rss_bytes": memory.peak_rss,
            "held": memory.held.iter().map(|(name, bytes)| serde_json::json!({
                "name": name,
                "bytes": bytes,
            })).collect::<Vec<_>>(),
        });
    }
    value
}
