# Replay a service

Every effect a Hirsel run performs is recorded, so a whole system of runs can be
replayed, cross-checked and tried against new code. This guide walks through
[examples/ledger-service](../examples/ledger-service/main.hir). It is a small ledger
whose store lives at one place and whose client calls it through a runner. The run
injects a crash in the middle of a commit and loses a reply. It then replays every run
from its tape, checks the links between the tapes, and replays the client's calls
against a fork of the store under a changed transfer function.

The rules are in [runners](spec/runners.md) and [storage](spec/storage.md).

## The service

`ledger.hir` holds the ledger's rules: `open` and `transfer` are transaction bodies
over a typed table `balances: Table[u64, u64]`. They hold only a `Tx` and data, so a
store can run them again. `main.hir` has two entries that run at the ledger's place,
`open_account(rt: Io, req: Opening)` and `transfer(rt: Io, req: Transfer)`. Each opens
the store over the place's `ledger` directory and runs one `store.call` on the client's
call stream:

```hirsel
fn transfer(rt: Io, req: Transfer) -> Result[ledger.Receipt, LedgerError] {
  let d = io.dir(rt, path.of("ledger")) ? Grant
  let mut s = store.open(d, store.default_config()) ? Store
  let outcome = store.call(
    mut s,
    req.call,
    fn(mut tx: store.Tx) -> Result[ledger.Receipt, ledger.Refusal] {
      ledger.transfer(mut tx, req.from, req.to, req.cents)
    }
  )
  store.close(take s) ? Store
  Ok(outcome ? Call)
}
```

The client in `main` numbers its calls by stream position. When a call's outcome is
`InDoubt`, it sends the same request again at the same position. The store runs each
position once (STORE-12). If the first attempt's commit was lost, the body runs again.
If the commit happened and only its reply was lost, the store returns the recorded
outcome without running anything. So every call takes effect exactly once, without
the client knowing which case it was in.

## Run it under a simulated place

The `ledger` runner is a `sim@1` node that models `remote.tls@1`. Its callees run
under a grant set that holds one directory, `db`. Copy the three grants templates to
a scratch directory, because relative `host` paths resolve beside the template:

```sh
mkdir -p /tmp/ledger/db && cp examples/ledger-service/*.json /tmp/ledger/
rm /tmp/ledger/hirsel.json
hirsel run examples/ledger-service/hirsel.json --grants /tmp/ledger/grants.json
```

That opens accounts 1 and 2 (positions 1 and 2). Fork the store now, before the next
calls. The store is a directory of immutable objects, so a copy is a consistent fork,
and opening it claims its own writer epoch:

```sh
cp -r /tmp/ledger/db /tmp/ledger/fork
```

`faults.json` turns on `sim@1`'s `crash` and `lose_reply` faults with
`inputs.fault_seed` 2071. Run the trading phase and record it:

```sh
hirsel run examples/ledger-service/hirsel.json --grants /tmp/ledger/faults.json --record /tmp/ledger/T
```

```text
3: in doubt, sending it again
3: moved 200 from 1 to 2, fee 0
4: opened 3 with 0
5: moved 50 from 2 to 3, fee 0
6: in doubt, sending it again
6: moved 250 from 2 to 1, fee 0
7: insufficient: 550
```

The first attempt at position 3 crashed its node just before the commit's log object
was written. The callee stops before one of its requests, chosen by the fault seed
(runners.md, `sim@1`). The store recovered without that commit, and the second attempt
ran the transfer. At position 6, the reply was lost after the commit. The second
attempt read the outcome the store had recorded for that position.

## Replay the whole system

`--record T` writes the client's tape to `T` and each callee run's tape to
`T.call-<n>`, where `n` is the request's ordinal. Every tape replays alone, with no
directory and no place:

```sh
hirsel run examples/ledger-service/hirsel.json --replay /tmp/ledger/T
hirsel run examples/ledger-service/hirsel.json --replay /tmp/ledger/T.call-0
```

The caller's replay prints the same seven lines. Each call's outcome comes from its
tape. The crashed callee's tape, `T.call-0`, ends with a `stopped` event where its place
stopped it. Its replay re-reads every object the store loaded, stops at the same point,
and ends with `environmental failure: its place stopped the run` (exit status 6), as the
live run did. The other callees replay to their recorded outcomes.

`hirsel tapes check` joins the set. For every link, the callee's argument bytes and
request digest must equal the caller's request. Where the caller received an outcome,
its bytes must equal what the callee recorded as it returned:

```sh
hirsel tapes check /tmp/ledger/T /tmp/ledger/T.call-*
```

```text
tapes: 7 links checked, 0 unchecked
```

## Try a changed function against the fork

`examples/ledger-service/trial` is a candidate revision of the same package. Only
`ledger.transfer` differs: it charges a 1% fee on top of the amount moved.
`hirsel tapes trial` takes every runner call the client's tape recorded and issues it
again, in order, to a callee of the candidate. Each call keeps its own entry, argument
bytes and cause, and runs under the runner grants of `fork.json`, which mount the fork:

```sh
hirsel tapes trial examples/ledger-service/trial/hirsel.json --grants /tmp/ledger/fork.json /tmp/ledger/T
```

```text
call 0 ledger_service@1/transfer (changed code): unrecorded
  recorded InDoubt(Abandoned())
  trial    Ok(Receipt(from: 1, to: 2, moved: 200, fee: 2))
call 1 ledger_service@1/transfer (changed code): DIVERGED
  recorded Ok(Receipt(from: 1, to: 2, moved: 200, fee: 0))
  trial    Ok(Receipt(from: 1, to: 2, moved: 200, fee: 2))
call 2 ledger_service@1/open_account (same code): same
  recorded Ok(0)
  trial    Ok(0)
call 3 ledger_service@1/transfer (changed code): same
  recorded Ok(Receipt(from: 2, to: 3, moved: 50, fee: 0))
  trial    Ok(Receipt(from: 2, to: 3, moved: 50, fee: 0))
call 4 ledger_service@1/transfer (changed code): unrecorded
  recorded InDoubt(Lost())
  trial    Failed(Call(Tx(Aborted(Insufficient(250)))))
call 5 ledger_service@1/transfer (changed code): DIVERGED
  recorded Ok(Receipt(from: 2, to: 1, moved: 250, fee: 0))
  trial    Failed(Call(Tx(Aborted(Insufficient(250)))))
call 6 ledger_service@1/transfer (changed code): DIVERGED
  recorded Failed(Call(Tx(Aborted(Insufficient(550)))))
  trial    Failed(Call(Tx(Aborted(Insufficient(298)))))
trial: 7 calls, first divergence at call 1
```

The trial issues the in-doubt attempts too, so each position runs once and its re-send
returns the trial's own recorded outcome, as in the live run. For each call, the trial
reports two things. `same code` means the candidate's callee
has the code hash and signature fingerprint that the recorded request named. `same`
means its outcome bytes equal the recorded ones. `open_account` does not reach the
changed function, so its calls match. Every `transfer` is changed code. The first one
diverges because of the fee. The 50-cent transfer still matches, since its fee rounds
to 0. The later calls diverge as the balances drift apart. `--from <n>` starts at
request `n`. The exit status is 1 when any call diverges, so a trial can gate an
upgrade.

## What is not here yet

- The callees open the store on every call, because the `db@1` provider and
  `runner.transact` (a transaction body run inside the place that holds the store) are
  reserved. Exactly-once still holds, because `store.call` decides each position
  inside the commit.
- A crash on a host directory keeps every write the callee completed. Only a
  simulated `Dir` loses unsynced writes.
- The fork is a copy of the store's directory taken between runs. Forking at any
  commit version, and the store's command log and verifier (STORE-12), are reserved.
