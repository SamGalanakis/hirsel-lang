# httpaggregate

Fetch 1–16 strict JSON item collections sequentially, sum u32 values by exact
name with checked u64 totals, and emit a bytewise-sorted JSON array only after
all endpoints succeed. Bodies, item counts and UTF-8 name lengths are bounded.

See [GUIDE.md](GUIDE.md) for narrow grants, build-once native execution and
offline record/replay with a local fixture server.
