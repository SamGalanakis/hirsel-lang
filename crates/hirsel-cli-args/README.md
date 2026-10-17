# hirsel-cli-args

Shared command-line parsing for the CLI and compiler tooling.

Allowed dependencies: lexopt and the standard library; no compiler phases.

- Paths remain OS strings, including non-UTF-8 paths.
- Common root, placement seed and shard options have one validation path.
- Argument errors print one message to stderr and exit 2.
- Defaults and which options a command accepts belong to its caller.
