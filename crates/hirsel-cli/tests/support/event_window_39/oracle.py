"""Independent integer/dictionary oracle and finite full-main vectors.

Beam fadfc789: window_test.py:98-116 and trigger_test.py:194-210.
The latter's (1,a),(2,b),(13,c) are adapted to explicit finite watermarks;
there is no implicit Beam runner end-of-input watermark. Apache-2.0, see
examples/eventwindow/{LICENSE,NOTICE}.beam. Expected bytes never come from Hirsel.
"""
import copy
import json
import sys

U64 = (1 << 64) - 1
I64_MIN, I64_MAX = -(1 << 63), (1 << 63) - 1


def canonical(state):
    return json.dumps(state, ensure_ascii=False, separators=(",", ":")) + "\n"


def empty(width=5, offset=2):
    return dict(schema=1, width=width, offset=offset % width, next_seq=0,
                watermark=None, watermarks=0, late=0, keys=[], live=[], emitted=[])


def event(seq, at, value=1, key="a"):
    return dict(seq=seq, key=key, at=at, value=value)


def mark(seq, at):
    return dict(seq=seq, watermark=at)


def stream(records):
    return "".join(canonical(r) for r in records)


def order(row):
    return row["end"], row["start"], row["key"].encode()


def apply(initial, records):
    s = copy.deepcopy(initial)
    live = {(r["start"], r["key"]): r for r in s["live"]}
    for r in records:
        assert r["seq"] == s["next_seq"] < U64
        if "watermark" in r:
            w = r["watermark"]
            assert s["watermark"] is None or w >= s["watermark"]
            s["watermark"] = w
            s["watermarks"] += 1
            closed = sorted((v for v in live.values() if v["end"] <= w), key=order)
            assert len(s["emitted"]) + len(closed) <= 128
            s["emitted"].extend(closed)
            for row in closed:
                del live[row["start"], row["key"]]
        else:
            k, at, value = r["key"], r["at"], r["value"]
            if k not in s["keys"]:
                assert len(s["keys"]) < 16
                s["keys"].append(k)
            if s["watermark"] is not None and at < s["watermark"]:
                s["late"] += 1
            else:
                start = ((at - s["offset"]) // s["width"]) * s["width"] + s["offset"]
                pair = start, k
                if pair not in live:
                    assert len(live) < 32
                    live[pair] = dict(start=start, end=start + s["width"], key=k,
                                      count=0, sum=0, min=value, max=value)
                row = live[pair]
                row["count"] += 1
                row["sum"] += value
                assert row["count"] <= U64 and I64_MIN <= row["sum"] <= I64_MAX
                row["min"] = min(row["min"], value)
                row["max"] = max(row["max"], value)
        s["next_seq"] += 1
    s["keys"].sort(key=lambda k: k.encode())
    s["live"] = sorted(live.values(), key=order)
    return s


def cases():
    out = []

    def add(id, records=(), initial=None, error=None, partition="semantics", **kw):
        initial = copy.deepcopy(initial if initial is not None else empty())
        row = dict(id=id, partition=partition, args=["apply", "state.json", "events.ndjson", "result.json"],
                   checkpoint=canonical(initial), input=stream(records), steps=5_000_000,
                   cpu_seconds=60, memory_bytes=2 * 1024**3, action_seconds=600,
                   exit=2 if error else 0, stderr=f"eventwindow: {error}\n" if error else "",
                   expected=None if error else canonical(apply(initial, records)))
        row.update(kw)
        out.append(row)
        return row

    def bad(id, records=(), **kw):
        return add(id, records, partition="invalid", **kw)

    for width, offset, timestamps in [(5, 2, [7, 11, 12]), (5, 0, [5, 9, 10]),
                                      (5, 12, [11]), (5, -3, [-9, -8, -4, -3, -1, 0, 1, 2])]:
        add(f"beam-{width}-{offset}", [event(i, at, i + 1) for i, at in enumerate(timestamps)], empty(width, offset))
    upstream = [event(0, 1, 1), event(1, 2, 2), event(2, 13, 3), mark(3, 10), mark(4, 20)]
    add("beam-watermark", upstream, empty(10, 0))
    # Upstream groupings 1,2,3,-3,-2,-1 are bundle sizes (negative reverses
    # event arrival), not watermark values: trigger_test.py:92-125,194-210.
    for grouping in (1,2,3,-3,-2,-1):
        data=[(1,1),(2,2),(13,3)]
        if grouping < 0:
            data.reverse()
        events=[event(i,at,value) for i,(at,value) in enumerate(data)]
        complete=events+[mark(3,10),mark(4,20)]
        final=add(f"beam-bundles-{grouping}",complete,empty(10,0),partition="beam")
        # Each prefix is an actual bounded batch boundary; the matching resume
        # case consumes those same canonical checkpoint bytes.
        for end in range(abs(grouping),len(events),abs(grouping)):
            prefix=add(f"beam-prefix-{grouping}-{end}",events[:end],empty(10,0),partition="beam")
            resumed=add(f"beam-resume-{grouping}-{end}",complete[end:],apply(empty(10,0),events[:end]),partition="beam")
            assert prefix["expected"] == resumed["checkpoint"]
            assert resumed["expected"] == final["expected"]
    records = [event(0, 12, 4, "z"), event(1, -1, -3, "é"), event(2, 7, 2, "a"),
               event(3, 8, 9, "a"), event(4, 7, -1, "Z"), mark(5, 12),
               event(6, 11, I64_MAX, "late"), event(7, 12, 6, "z"), mark(8, 12), mark(9, 17)]
    full = add("out-of-order", records)
    for split in (0, 1, 5, 6, 7, len(records)):
        resumed = add(f"split-{split}", records[split:], apply(empty(), records[:split]),
                      prefix=stream(records[:split]), full=full["expected"])
        prefix = add(f"prefix-{split}", records[:split])
        assert prefix["expected"] == resumed["checkpoint"]
        assert resumed["expected"] == full["expected"]
    add("empty")
    add("no-eof-flush", [event(0, 7), event(1, -8)])
    add("first-watermark", [event(0, -1000000000), mark(1, -1000000000),
                             event(2, -1000000000), mark(3, -999999997)])
    add("tick-ends", [event(0, -1000000000, I64_MIN), event(1, 1000000000, I64_MAX)], empty(3600, -1000000000))
    add("escaped-byte-order", [event(i, 0, i, k) for i, k in enumerate(["é", "z", "Z", "\x00", "", "\\\""])]+[mark(6, 2)])
    for width, offset in [(1, -1000000000), (3600, 1000000000), (5, -3)]:
        r = add(f"init-{width}-{offset}")
        r.update(args=["init", str(width), str(offset), "result.json"], expected=canonical(empty(width, offset)))
    bad("duplicate", [event(0, 0), event(0, 0)], error="sequence")
    bad("gap", [event(1, 0)], error="sequence")
    bad("decrease", [mark(0, 1), mark(1, 0)], error="watermark_decrease")
    for id, raw, error in [
        ("unknown", '{"seq":0,"key":"a","at":0,"value":1,"x":0}\n', "shape"),
        ("duplicate-json", '{"seq":0,"seq":0,"watermark":0}\n', "json"),
        ("missing", '{"seq":0,"key":"a","at":0}\n', "shape"),
        ("both", '{"seq":0,"key":"a","at":0,"value":1,"watermark":0}\n', "shape"),
        ("blank", '\n', "json"), ("malformed", '{', "json"),
        ("fraction", '{"seq":0,"key":"a","at":0,"value":1.0}', "integer"),
        ("exponent", '{"seq":0,"key":"a","at":0,"value":1e0}', "integer"),
        ("negative-seq", '{"seq":-1,"watermark":0}', "integer")]:
        bad(id, error=error, input=raw)
    for name, rec in [("tick-high", event(0, 1000000001)), ("tick-low", mark(0, -1000000001)),
                      ("value-over", event(0, 0, I64_MAX+1))]:
        bad(name, error="integer" if name == "value-over" else "tick", input=stream([rec]))
    base = apply(empty(), [event(0, 7, 2), event(1, 12, 3), mark(2, 12)])
    mutations = {
        "version": ("schema", 2, "schema"), "width": ("width", 0, "width"),
        "offset": ("offset", 7, "offset"), "seq": ("next_seq", 0, "accounting"),
        "wm-range": ("watermark", 1000000001, "tick"),
        "wm-none": ("watermark", None, "watermark_count"),
        "wm-count": ("watermarks", 0, "watermark_count"),
        "late": ("late", 1, "accounting"),
        "keys-order": ("keys", ["b", "a"], "key_order"),
        "keys-duplicate": ("keys", ["a", "a"], "key_order"),
    }
    for name, (field, value, error) in mutations.items():
        s = copy.deepcopy(base); s[field] = value
        bad("tamper-"+name, error=error, checkpoint=canonical(s))
    for name, field, val, error in [
        ("end", "end", 99, "interval"), ("alignment", "start", 13, "interval"),
        ("key", "key", "unknown", "unknown_key"), ("count", "count", 0, "aggregate"),
        ("sum", "sum", 4, "aggregate"), ("min", "min", 4, "aggregate"),
        ("max", "max", 2, "aggregate")]:
        s=copy.deepcopy(base); s["live"][0][field]=val
        bad("tamper-"+name, error=error, checkpoint=canonical(s))
    s=copy.deepcopy(base); s["live"].append(copy.deepcopy(s["live"][0]))
    bad("tamper-duplicate-window", error="window_order", checkpoint=canonical(s))
    s=copy.deepcopy(base); s["live"],s["emitted"]=s["emitted"],s["live"]
    bad("tamper-window-watermark", error="window_watermark", checkpoint=canonical(s))
    s=empty(); s["unused"]=0
    bad("tamper-extra", error="shape", checkpoint=canonical(s))
    s=empty(); del s["late"]
    bad("tamper-missing", error="shape", checkpoint=canonical(s))
    s=empty(); s["keys"]=["a"]
    bad("tamper-key-accounting", error="key_accounting", checkpoint=canonical(s))
    seed=apply(empty(), [event(0, 0, I64_MAX)])
    bad("sum-overflow", [event(1, 0, 1)], initial=seed, error="overflow")
    seed=apply(empty(), [event(0, 0, I64_MIN)])
    bad("sum-underflow", [event(1, 0, -1)], initial=seed, error="overflow")
    seed=empty(); seed.update(next_seq=U64, watermarks=1, late=U64-1, watermark=0, keys=["a"])
    bad("seq-overflow", [mark(U64, 0)], initial=seed, error="overflow")
    seed["late"]=U64
    bad("account-overflow", error="overflow", checkpoint=canonical(seed))
    seed=empty(); seed.update(next_seq=U64-1, watermarks=1, late=U64-2, watermark=0, keys=["a"])
    add("seq-maximum", [mark(U64-1,0)], initial=seed)
    seed["keys"]=[]
    bad("tamper-late-registry", error="key_accounting", checkpoint=canonical(seed))
    for id, member in [("utf8-event","input_bytes"),("utf8-checkpoint","checkpoint_bytes")]:
        bad(id,error="utf8",**{member:[255]})
    for n in (128,129):
        bad(f"json-depth-{n}",error="shape" if n==128 else "json",input="["*n+"]"*n)
    for id, args, error in [("width-zero",["init","0","0","result.json"],"width"),
                             ("width-over",["init","3601","0","result.json"],"width"),
                             ("offset-over",["init","5","1000000001","result.json"],"tick"),
                             ("usage",[],"usage"),
                             ("command",["other","a","b","result.json"],"usage")]:
        bad(id,error=error,args=args)
    seed=apply(empty(),[event(0,0),event(1,7)])
    seed["live"].reverse()
    bad("tamper-live-order",error="window_order",checkpoint=canonical(seed))
    seed=apply(empty(),[event(0,0),event(1,7),mark(2,12)])
    seed["emitted"].reverse()
    bad("tamper-history-order",error="window_order",checkpoint=canonical(seed))
    seed=apply(empty(),[event(0,0),event(1,0),mark(2,2)])
    seed["keys"].append("unused")
    add("tamper-unused-registry",error="key_accounting",checkpoint=canonical(seed),partition="registry")
    seed=apply(empty(),[mark(0,2),event(1,0,key="late-only")])
    add("late-only-registry",initial=seed,partition="registry")
    for n in (16,17):
        add(f"keys-{n}", [event(i, 0, key=str(i)) for i in range(n)], partition="caps",
            error="keys_limit" if n==17 else None)
    for n in (32,33):
        add(f"live-{n}", [event(i, i) for i in range(n)], empty(1,0), partition="caps",
            error="live_limit" if n==33 else None)
    for n in (128,129):
        add(f"records-{n}", [mark(i,0) for i in range(n)], partition="caps",
            error="records_limit" if n==129 else None)
    for n in (32,33):
        add(f"key-bytes-{n}", [event(0,0,key="é"*16+("x" if n==33 else ""))],
            partition="caps", error="key_limit" if n==33 else None)
    for n in (1024,1025):
        text=canonical(event(0,0)).rstrip("\n").ljust(n)+"\n"
        add(f"line-bytes-{n}", [event(0,0)], partition="caps", input=text,
            error="line_limit" if n==1025 else None)
    # 32 x 1023-byte lines + LF = the byte cap. The extra LF refuses before
    # feeding the scanner, independent of a later blank-line parse error.
    text="".join(canonical(mark(i,0)).rstrip("\n").ljust(1023)+"\n" for i in range(32))
    for n in (32768,32769):
        add(f"input-bytes-{n}", [mark(i,0) for i in range(32)], partition="caps", input=text+("\n" if n==32769 else ""), error="input_limit" if n==32769 else None)
    for n in (65536,65537):
        add(f"checkpoint-bytes-{n}", partition="caps", checkpoint=canonical(empty()).ljust(n),
            error="checkpoint_limit" if n==65537 else None)
    for n in (128,129):
        seed=apply(empty(1,0), [event(i,i) for i in range(32)]+[mark(32,32)])
        for k in range(1,4):
            seq=seed["next_seq"]
            seed=apply(seed, [event(seq+i,k*32+i) for i in range(32)]+[mark(seq+32,(k+1)*32)])
        if n==128:
            add("emitted-128", initial=seed, partition="caps")
        else:
            add("emitted-129", [event(seed["next_seq"],128),mark(seed["next_seq"]+1,129)], seed, error="emitted_limit",partition="caps")
    s=copy.deepcopy(seed); s["emitted"].append(copy.deepcopy(s["emitted"][-1]))
    add("tamper-retained-cap", error="window_limit", checkpoint=canonical(s),partition="caps")
    s=empty(); s["live"]=[{}]*33
    add("tamper-live-cap", error="window_limit", checkpoint=canonical(s),partition="caps")
    s=empty(); s["keys"]=[str(i) for i in range(17)]
    add("tamper-keys-cap", error="keys_limit", checkpoint=canonical(s),partition="caps")
    for operation, partial in [("file.open",0),("file.read",0),("file.create_atomic",0),("file.write_all",3),("file.commit",0)]:
        add("fault-"+operation.split(".")[1], partition="faults", error="file",
            faults={operation:[dict(requested=4096 if operation=="file.read" else len(canonical(empty()).encode()) if operation=="file.write_all" else None,error="PermissionDenied",partial=partial)]})
    add("pending-interrupt", partition="faults", error="canceled", pending=True)
    add("output-denied", partition="faults", error="output_grant", deny_output=True)
    add("crlf", [event(0,0)], input=stream([event(0,0)]).replace("\n","\r\n"))
    add("no-final-lf", [event(0,0)], input=stream([event(0,0)]).rstrip("\n"))
    # Final scanner generation: physical 256-byte refill boundaries and
    # admission-before-parse precedence, independently of event-time math.
    records=[event(0,0),event(1,12,2)]
    raw=canonical(records[0]).rstrip("\n").ljust(255)+"\r\n"+canonical(records[1])
    add("scanner-crlf-split",records,input=raw,partition="scanner")
    raw=canonical(records[0]).rstrip("\n").ljust(255)+"\n"+canonical(records[1])
    add("scanner-lf-edge",records,input=raw,partition="scanner")
    prefix='{"seq":0,"key":"'
    raw=' '*(255-len(prefix.encode()))+prefix+'é","at":0,"value":3}\n'
    add("scanner-utf8-split",[event(0,0,3,"é")],input=raw,partition="scanner")
    add("scanner-blank",error="json",input="\n",partition="scanner")
    add("scanner-empty",partition="scanner")
    add("scanner-no-final-lf",[event(0,0)],input=stream([event(0,0)]).rstrip("\n"),partition="scanner")
    add("scanner-no-eof-flush",[event(0,0),event(1,12)],partition="scanner")
    add("scanner-line-precedence",error="line_limit",input=" "*1025+"\n\n",partition="scanner")
    add("scanner-json-precedence",error="json",input="!\n"+" "*1025+"\n",partition="scanner")
    records=[mark(i,0) for i in range(128)]
    add("scanner-record-precedence",records,error="records_limit",input=stream(records)+"\n",partition="scanner")
    return out


if __name__ == "__main__":
    json.dump(cases(), sys.stdout, ensure_ascii=False)
