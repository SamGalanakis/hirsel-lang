Run the deterministic echo demo from the repository root:

```sh
hirsel run --simulation examples/fetchjson/demo.json examples/fetchjson/hirsel.json
```

It prints `0` and `1` on separate lines. Both JSON POSTs use one `http.Client`
and one persistent connection. `http.json_body` supplies the media type, `client_send`
sets Content-Type, and `response_json` decodes each reply with std JSON.

For a real JSON echo service, edit `grants.json` to grant its exact host and port,
then run:

```sh
hirsel run examples/fetchjson/hirsel.json --grants examples/fetchjson/grants.json -- http://your-host/echo
```

The explicit monotonic clock supplies the client's 30-second idle expiry.
Whole-request deadlines use `task.scope_until` around a call, as the HTTP module
and [std documentation](../../docs/spec/std-library.md#http-client) show.
