# Typed JSON kit

This is a hand-authored library fixture, with no entry point or host grants.
Generate its missing `hooks.hir`, then check and run its four tests:

```sh
tools/json-hooks Item Snapshot --root Snapshot > examples/typed-json-kit/hooks.hir && hirsel check examples/typed-json-kit/check.json && hirsel test examples/typed-json-kit/hirsel.json
```

Run from the repository root. The generated file is ignored and must not be
committed. See [the kit guide](../../docs/typed-json-kit.md) for all five exact
declarations, nested type lists, native tests and imported nominal wiring.
