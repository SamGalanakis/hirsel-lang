# Confined filesystem lifecycle

Creates nested output directories, lists their direct entries (including an empty
directory), reads metadata, renames the tree and removes the created entries.
All paths stay under one explicitly granted `Dir`.

Supply a writable empty host directory as the `output` mount:

```json
{"io":{"stdout":"stdout.bytes@1","dirs":[{"path":"output","host":"/absolute/output","access":"read_write"}]}}
```

```sh
hirsel run examples/filesystem/hirsel.json --grants grants.json
hirsel build examples/filesystem/hirsel.json -o filesystem
./filesystem --hirsel-grants grants.json
```

The CLI prints `build/empty` and `build/value` in UTF-8 byte order with their
entry kinds and metadata sizes; directory sizes are provider inputs. Metadata timestamps are
filesystem inputs and do not need a clock grant. As with other file operations,
a failure can leave already completed changes in the output directory.
