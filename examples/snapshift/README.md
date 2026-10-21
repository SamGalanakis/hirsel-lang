# Typed JSON snapshot transformer

Keep active items in their original order and preserve the capture date. Input is
at most 64 KiB. Decode directly from bytes, including checked `time.Date`, then
write compact JSON directly from records. Unknown fields are ignored; missing
required fields, duplicate keys and invalid dates fail before stdout is written.

```sh
hirsel test examples/snapshift/hirsel.json
printf '%s' '{"captured":"2024-02-29","items":[{"name":"wool","active":true}]}' | hirsel run examples/snapshift/run.json --grants examples/snapshift/grants.json
hirsel run examples/snapshift/run.json --native --backend llvm --grants examples/snapshift/grants.json < snapshot.json
hirsel run examples/snapshift/run.json --native --backend cranelift --grants examples/snapshift/grants.json < snapshot.json
python3 examples/verify-dogfood6.py --hirsel "$(command -v hirsel)" --tool snapshift
```

This began with `hirsel new snapshift --lib`: `hirsel.json` has no entry and
exports `transform`. `run.json` adds a CLI driver to the same package. Use that
launch manifest for `check` and `run`; `test` accepts the library manifest.
Three tests cover filtering, Date validation, duplicates and escaped names.
