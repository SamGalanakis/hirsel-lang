# tasks

A task manager served over HTTP with [Datastar](https://data-star.dev). The server
renders the HTML, and the browser keeps one event stream open that morphs the page
whenever any browser changes the board. The board lives in memory and is saved as
snapshots in a granted directory, so it survives a restart.

- Tasks have a title, a description, a status (to do, doing, done), a priority, a
  due date and tags.
- Create from the form at the top. Edit inline. Advance the status by clicking it.
  Move up or down, or delete.
- Filter by status and by tag, search title, description and tags, sort by your
  order, due date, priority or title, and open a task's detail panel (`/tasks/<id>`
  links to it).
- Every open browser sees each change at once, over its `GET /updates` stream.

The server uses the [datastar@1](../../packages/datastar@1/README.md) SDK through
its manifest. The page loads the vendored Datastar v1.0.4 client from
`static/datastar.js`.

## Running it

```sh
examples/tasks/run          # http://127.0.0.1:8080/
examples/tasks/run 9000     # another port
examples/tasks/run 8080 --native   # compiled with LLVM first
```

`run` writes `data/grants.json` from `grants.json` and starts
`hirsel run hirsel.json --grants data/grants.json`. The grants bind the port, the
`data` directory (read-write, the snapshots) and `static` (read-only). With the
monotonic clock granted, a request must arrive within 30 s; an idle keep-alive
connection closes after 120 s either way. Press Ctrl-C or send SIGTERM to stop
the server gracefully. `quit` and Ctrl-D remain available for the end-to-end
tests and scripted launches. It stops accepting and closes its
connections, which ends every event stream. Then it waits for the writer to save
every snapshot it was given, and exits.

### Configuration

Flags win over environment variables, which win over the defaults:

| Flag | Environment | Default | |
|---|---|---|---|
| `--host` | `TASKS_HOST` | `127.0.0.1` | listen address (the grant must allow it) |
| `--port`, `-p` | `TASKS_PORT` | `8080` | listen port, `0` for any |
| `--durability` | `TASKS_DURABILITY` | `volatile` | `durable` makes each save survive a crash before it is acknowledged |
| `--log-level` | `TASKS_LOG_LEVEL` | `info` | `debug`, `info`, `warn` or `error` |

`--help` prints these. Logs are JSON lines on stderr: `loaded`, `listening`,
`request` (method, path, status), `stream`, `stopping` (with the reason) and
`stopped`. Each line carries a timestamp when the wall clock is granted. The
wall clock also gives the "due in 3 days" texts; without it, dates show as they
are.

## How it is built

| Module | |
|---|---|
| `model.hir` | Tasks and the board: validation, mutations and the filtered, sorted view. All of it is pure. |
| `store.hir` | Persistence, the one module that knows the storage format. Each change is a new `board-<revision>.json` written with `file.put_new`; loading takes the newest snapshot that parses. |
| `view.hir` | Server-rendered HTML. Every text from a task goes through std's `html.esc`. |
| `httpd.hir` | Replies, limits and request helpers over std's HTTP/1.1 server connection. |
| `hub.hir` | The broadcast: a task that tells every open event stream about each new revision. |
| `app.hir` | Routing, handlers, the writer task and `serve`. |
| `main.hir` | Grants and configuration. |

A change is applied under the board's mutex. Its snapshot then goes to the
writer task, which saves it and acknowledges. Only after that is the new revision
published to the hub, and only then does the request get its answer. So a
browser never sees a change that was not saved. Each event stream re-renders its
own browser's view (its filters and its selected task) for each new revision and
sends it as one `datastar-patch-elements` event.

The hub's channel sends never check cancellation: an open stream receives the
revision even if the producer was canceled, and a closed stream returns `Closed`.
The hub stops at its next `channel.receive`, which checks cancellation. A producer
loop that only sends should call `task.check_canceled(s)?` before each send.

Actions that take input answer with Datastar signal patches: create clears the
form, edit closes it, and an invalid draft sets `$error`. The other actions
answer `204`, and the stream carries the result.

## HTTP contract

These routes and behaviors are what the end-to-end suite checks. Another
implementation that serves them passes it: an implementation in another language
can be checked with the same suite.

| Route | Answer |
|---|---|
| `GET` or `HEAD /` | `200 text/html; charset=utf-8`, the whole page. Query `status` (`todo`, `doing`, `done`; others ignored), `tag`, `q` (search) and `sort` (`manual`, `due`, `priority`, `title`; others mean `manual`) preselect the filters. |
| `GET` or `HEAD /tasks/<id>` | The page with that task's detail panel open, or `404` HTML when there is no such task. An id is a positive integer. |
| `GET /updates?datastar=<signals>` | `200 text/event-stream`, kept open. Signals: `{"filter":{"status","tag","text","sort"},"selected":<id>}`. One `datastar-patch-elements` event now and one per later change, each carrying `#stats`, `#tag-filter`, `#list` and `#detail` for that view. The stream ends when the server stops. |
| `POST /tasks` | Datastar signals as a JSON body, `{"draft":{"title","description","priority","due","tags"}}` (other signals are ignored). A valid draft gets `200 text/event-stream` with one `datastar-patch-signals` event: `signals {"draft":{"title":"","description":"","priority":"medium","due":"","tags":""},"error":""}`. An invalid one gets the same with `signals {"error":"<why>"}` and changes nothing. A body that is not the signals gets `400`. |
| `POST /tasks/<id>` | An `application/x-www-form-urlencoded` edit with fields `title`, `description`, `priority`, `due` and `tags`; the status is kept. Success is `signals {"editing":0,"error":""}`, invalid input is an error signal as above, any other content type is `415`, and a missing task is `404`. |
| `POST /tasks/<id>/advance` | Status todo → doing → done → todo. `204`, or `404`. |
| `POST /tasks/<id>/move?dir=up\|down` | Swaps with the neighbour in the manual order. `204`, also at either end, where nothing changes and no event is sent. `404` for a missing task or another `dir`. |
| `DELETE /tasks/<id>` | `204`, or `404`. |
| `GET` or `HEAD /static/<name>` | The file with its content type and an `ETag`; a matching `If-None-Match` gets `304`. |
| anything else | `404` HTML. A known path with another method gets `405` with `Allow`. |

Validation trims the title, which is required and at most 200 bytes. The
description is at most 4000 bytes. Priority is `low`, `medium` or `high`, and
empty means medium. The due date is empty or a valid `YYYY-MM-DD`. Tags are comma
separated, trimmed, lowercased and deduplicated: at most 8, each at most 32 bytes
of letters, digits, `-` and `_`. Every change bumps the board's revision; a
change that alters nothing (a move at either end) does not. Ids are never
reused, also across restarts. When saving a change fails the answer is `500`;
during shutdown it is `503`.

The suite reads the markup it checks from `view.hir`: the element ids above,
`<li id="task-<id>" class="task status-<status> priority-<priority>">`, the escaped
title in the row's link, the due date's `datetime` attribute and the stats line. It
does not depend on the server's clock.

## Tests

```sh
hirsel test examples/tasks/hirsel.json            # 22 unit tests; add --native for LLVM
```

They cover validation, mutations and sorting; snapshots across simulated crashes
(`test.crash`) and corrupt files; HTML escaping and the live patch; request limits, forms and
reply framing; routing; configuration; and, on simulated tasks (`test.tasks`),
the hub's broadcast and a change being saved before any stream hears of it.

`e2e/` is the end-to-end suite: a Hirsel client that knows only the routes above
and talks to a running server through std's HTTP client. Its phases are:

- `first <port>`, against an empty board: CRUD through HTTP, the events each
  action sends on its reply and on the stream, and a broadcast reaching a second,
  filtered stream;
- `watch <port>`: holds a stream until the server stops, which must end it;
- `second <port>`, after a restart over the same data: the board, ids and
  revisions as the first run left them.

`crates/hirsel-cli/tests/examples.rs` (`tasks_end_to_end_*`) starts the server
process, runs the three phases, stops it with stdin EOF, restarts it on the same
port and checks the snapshot files. It does this with the server interpreted and
built with LLVM; the client runs interpreted. It also runs the unit tests on both native
backends. By hand:

```sh
examples/tasks/run 8080 &          # then, on an empty board:
hirsel run examples/tasks/e2e/hirsel.json \
  --grants '{"io":{"stdout":"stdout.bytes@1","dirs":[],"net":{"provider":"net.stream@1","rules":[{"address":"127.0.0.1","service":"8080","operation":"connect"}]}}}' \
  -- first 8080
```

## Next

There is no authentication: anyone who can reach the port can change the board.
The next increment is accounts with sessions (signed cookies) and a per-user
board. GAPS.md lists what the language and std do not provide yet, and the
workaround for each.
