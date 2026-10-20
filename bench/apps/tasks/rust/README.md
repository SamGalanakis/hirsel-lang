# Rust task manager baseline

Standalone axum + tokio implementation of `examples/tasks`. Build with
`cargo build --release --locked --manifest-path bench/apps/tasks/rust/Cargo.toml`.
Run from `examples/tasks` (with a `data` directory), or select directories through
`TASKS_DATA` and `TASKS_STATIC`. The four app flags and environment variables are
the Hirsel app's, with the same precedence and defaults. Stdin `quit`/EOF or
SIGINT/SIGTERM stops acceptance, ends SSE streams and drains persistence.

`TASKS_RUNTIME=current_thread` (default) or `multi_thread` selects tokio's scheduler;
the latter always has one worker. `TASKS_WALL_CLOCK=off` disables relative dates
and log timestamps, matching the e2e client's server grants. Snapshot storage is
selected with `TASKS_STORE=files` (default); durability is `volatile` or `durable`.
`TASKS_STORE=memory` is the simulated provider for persistence-failure isolation,
never used in comparative results. Both providers implement the same store seam.
No process-global allocator override. Static assets are the original files.

The official Datastar SDK supplies SSE encoding. Rendering follows `view.hir`,
including its ids, attributes, signals and escaping. Persistent JSON is compatible
in both directions (PascalCase enum strings, nullable dates, manual-order array).
One serialized writer publishes only after saving; failed saves retain the
in-memory mutation, as in Hirsel. Watch receivers coalesce revisions under
backpressure, as does the Hirsel hub. The Rust HTTP parser is Hyper's; malformed
wire requests can have different error wording/framing from Hirsel's parser.

Run `../e2e --hirsel /absolute/path/to/hirsel` from this directory, or invoke it
from the repository root as `bench/apps/tasks/e2e`. It executes the unchanged
Hirsel HTTP suite, including restart and stream shutdown, against this server.
`../bench` builds and measures the implementations; see the parent README.
