# Local HTTP mirror

Fetch one URL under `http://127.0.0.1:18766/` into a path relative to the `data`
mount. Responses are limited to 64 KiB; redirects and decompression are disabled.
An HTTP error leaves the destination unchanged. Successful writes replace it.
Filesystem errors propagate; destination paths gain no authority of their own.

```sh
mkdir -p examples/httpmirror/data
hirsel test examples/httpmirror/hirsel.json
hirsel run examples/httpmirror/hirsel.json --grants examples/httpmirror/grants.json
hirsel run examples/httpmirror/hirsel.json --native --backend llvm --grants examples/httpmirror/grants.json
hirsel run examples/httpmirror/hirsel.json --native --backend cranelift --grants examples/httpmirror/grants.json
python3 examples/verify-dogfood6.py --hirsel "$(command -v hirsel)" --tool httpmirror
```

The shipped grant uses a local scripted HTTP peer returning four binary bytes;
no server is needed. For a real loopback server, replace `io.net` with
`{"provider":"net.stream@1","rules":[{"address":"127.0.0.1","service":"18766","operation":"connect"}]}`.
The four tests cover binary storage, HTTP status errors, directory confinement and
URL refusal. The verifier records a successful write and a refused `../` write;
replay confirms the write without changing a sentinel file on the host.
