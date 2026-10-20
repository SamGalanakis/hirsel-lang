------------------------------ MODULE Ordered ------------------------------
(***************************************************************************)
(* Ordered mode (`tasks.cooperative@1` at width > 1): turns, the           *)
(* retirement frontier, run-ahead (refinement 1) and early commit          *)
(* (refinement 2), as specified in the multicore design, section 4.3.      *)
(*                                                                         *)
(* One state machine serves both sides of the check. Its head step is a    *)
(* function: the head task runs its next op, and a turn's end runs the     *)
(* fifo@1 scheduling point. The width-1 reference is that head step        *)
(* iterated to the end of the run (RunSerial), computed once per program.  *)
(* The executor under test adds the steps of every other queued task:      *)
(* logging deferrable ops, committing early-eligible ops early, charging   *)
(* budget pieces, and pausing. Any worker may take any step at any time,   *)
(* so every width and every interleaving is a behavior of Spec.            *)
(*                                                                         *)
(* Invariants (a)-(c) of section 4.3 Soundness compare the retired turns,  *)
(* the per-object sub-event histories, the pending messages and the        *)
(* logical queue against the reference.                                    *)
(***************************************************************************)
EXTENDS Naturals, Integers, Sequences, FiniteSets, TLC

(* Cfg: one scenario (see MC.tla). Mut: the protocol rules a mutation run  *)
(* removes; empty for the real checks. Eager: non-head steps take priority *)
(* over the head and early commit whenever a rule allows it (positive      *)
(* traces).                                                                *)
CONSTANTS Cfg, Mut, Eager

Tasks == 1 .. Cfg.n
Big == 1000000

-----------------------------------------------------------------------------
(* Ops. Every op is a record with the same fields so that TLC compares     *)
(* them freely: k (kind), h (handle), v (value), t (task), d (a handle the *)
(* message carries).                                                       *)

Mk(k, h, v, t, d) == [k |-> k, h |-> h, v |-> v, t |-> t, d |-> d]
Store(h, v)   == Mk("store", h, v, 0, "")
Load(h)       == Mk("load", h, 0, 0, "")
Send(h, v)    == Mk("send", h, v, 0, "")
SendH(h, d)   == Mk("send", h, 0, 0, d)
Recv(h)       == Mk("recv", h, 0, 0, "")
Close(h)      == Mk("close", h, 0, 0, "")
DropRecv(h)   == Mk("droprecv", h, 0, 0, "")
Take(h)       == Mk("take", h, 0, 0, "")
Put(h, v)     == Mk("put", h, v, 0, "")
Thaw(h)       == Mk("thaw", h, 0, 0, "")
Release(h)    == Mk("release", h, 0, 0, "")
CancelT(t)    == Mk("cancel", "", 0, t, "")
Check         == Mk("check", "", 0, 0, "")
HostReq(h)    == Mk("host", h, 0, 0, "")
Emit(v)      == Mk("print", "", v, 0, "")
Fail          == Mk("fail", "", 0, 0, "")
Detach(t)     == Mk("detach", "", 0, t, "")
DetachAll     == Mk("detachall", "", 0, 0, "")
Hold          == Mk("hold", "", 0, 0, "")
Work          == Mk("work", "", 0, 0, "")
Alloc(n)      == Mk("alloc", "", n, 0, "")
Free(n)       == Mk("free", "", n, 0, "")
Fuel(n)       == Mk("fuel", "", n, 0, "")
NoOp          == Mk("none", "", 0, 0, "")

(* Segment work: charges and pure computation, not ops (locality L).       *)
IsCharge(op) == op.k \in {"alloc", "free", "fuel", "work"}

HObj(h) == Cfg.hobj[h]
Trivial(c) == Cfg.fin[c] = NoOp

(* Section 4.1: D (deferrable) ops. A Receiver drop is D only when its     *)
(* payload is trivially droppable; otherwise it is R (its leftovers).      *)
DKinds == {"store", "put", "close", "release", "cancel", "detach", "detachall"}
IsD(op) == \/ op.k \in DKinds
           \/ op.k = "droprecv" /\ (Trivial(HObj(op.h)) \/ "droprecvD" \in Mut)

(* The set E of refinement 2 (the model's subset: no create ops).          *)
EKinds == {"store", "load", "send", "take", "put"}
            \cup (IF "cancelE" \in Mut THEN {"cancel"} ELSE {})
            \cup (IF "recvE" \in Mut THEN {"recv"} ELSE {})

(* The transition of a D op, known before it commits.                      *)
StaticTr(op) == CASE op.k \in {"store", "put", "cancel"} -> "yield"
                  [] op.k \in {"detach", "detachall"} -> "call"
                  [] OTHER -> "keep"

Ends(tr) == tr \in {"yield", "yieldpark", "park", "call", "hold", "fail"}

(* The footprint items an op touches: its object, and for cancellation and *)
(* receives the cancel state (an item no possible footprint holds).        *)
Items(op) == (IF op.h # "" THEN {HObj(op.h)} ELSE {})
               \cup (IF op.k \in {"cancel", "recv", "check"} THEN {"cancel"} ELSE {})

-----------------------------------------------------------------------------
(* Helpers. *)

Range(f) == {f[i] : i \in DOMAIN f}
Min(S) == CHOOSE x \in S : \A y \in S : x <= y
Last(q) == q[Len(q)]
IndexOf(q, x) == CHOOSE i \in 1 .. Len(q) : q[i] = x
Ev(t, o, k, out) == [t |-> t, o |-> o, k |-> k, out |-> out]
IsPrefix(a, b) == Len(a) <= Len(b) /\ a = SubSeq(b, 1, Len(a))
Children(t) == {u \in Tasks : Cfg.parent[u] = t}
RECURSIVE Desc(_)
Desc(t) == {t} \cup UNION {Desc(c) : c \in Children(t)}
Insert(q, i, ins) == SubSeq(q, 1, i - 1) \o ins \o SubSeq(q, i, Len(q))
Str(n) == ToString(n)

NoWait == [k |-> "", h |-> "", o |-> 0, at |-> 0, issued |-> 0]
EmptyPiece == [f |-> 0, n |-> 0, env |-> <<>>]

-----------------------------------------------------------------------------
(* The initial state. *)

Objects == DOMAIN Cfg.q0
Holders(x) == {<<t, h>> \in Tasks \X DOMAIN Cfg.hobj : h \in Cfg.held[t] /\ HObj(h) = x}
Obj0 == [x \in Objects |->
          [val |-> 0, holder |-> 0, q |-> Cfg.q0[x],
           senders |-> Cardinality({p \in Holders(x) : p[2] \in Cfg.senders}),
           rx |-> \E p \in Holders(x) : p[2] \notin Cfg.senders,
           count |-> Cardinality(Holders(x))]]

InitTask(p, t) ==
  [prog |-> p[t], pc |-> 1, ord |-> 0, st |-> Cfg.st0[t], wait |-> Cfg.wait0[t],
   parkNext |-> FALSE, log |-> <<>>, pend |-> FALSE, held |-> Cfg.held[t],
   canceled |-> FALSE, detached |-> FALSE, handoff |-> 0, calls |-> <<>>,
   callYield |-> FALSE, callLogged |-> FALSE]

InitState(p, memlim, fuellim) ==
  [obj |-> Obj0, hist |-> [x \in Objects |-> <<>>],
   tk |-> [t \in Tasks |-> InitTask(p, t)],
   head |-> Head(Cfg.queue), lq |-> Tail(Cfg.queue),
   parked |-> Cfg.parked0, parks |-> Len(Cfg.parked0), woken |-> {}, touched |-> {},
   tick |-> 0, sps |-> 0, turns |-> <<>>, cur |-> <<>>, polls |-> <<>>, out |-> <<>>,
   level |-> Cfg.level0, fuel |-> 0, memlim |-> memlim, fuellim |-> fuellim,
   spec |-> 0, stop |-> "none", early |-> {}, fault |-> "", p |-> p]

-----------------------------------------------------------------------------
(* Cancellation (writes cancel states) and scheduling points.              *)

CancelTree(s, t) ==
  LET sub == Desc(t) IN
  [s EXCEPT !.tk = [u \in Tasks |-> IF u \in sub THEN [s.tk[u] EXCEPT !.canceled = TRUE]
                                                  ELSE s.tk[u]],
            !.woken = @ \cup (sub \cap Range(s.parked))]

RECURSIVE CancelAll(_, _)
CancelAll(s, ts) == IF ts = {} THEN s
                    ELSE LET u == CHOOSE u \in ts : TRUE
                         IN CancelAll(CancelTree(s, u), ts \ {u})

HostReady(s, w) ==
  LET wt == s.tk[w].wait
  IN wt.k = "host" /\ Cfg.delay[wt.h] >= 0 /\ s.tick - wt.issued >= Cfg.delay[wt.h]

(* wake_receivers: the receivers parked on c, in park order, as many as c  *)
(* holds visible (non-pending) messages, canceled ones taking none; all of *)
(* them once no sender is left.                                            *)
RECURSIVE WakeLoop(_, _, _, _, _, _)
WakeLoop(s, ws, served, queued, closed, acc) ==
  IF ws = <<>> \/ (~closed /\ served = queued) THEN acc
  ELSE LET w == Head(ws)
       IN WakeLoop(s, Tail(ws), IF s.tk[w].canceled THEN served ELSE served + 1,
                   queued, closed, acc \cup {w})

Visible(m) == ~m.pend

WakeRecv(s, c) ==
  LET ws == SelectSeq(s.parked, LAMBDA w : s.tk[w].wait.k = "recv"
                                           /\ HObj(s.tk[w].wait.h) = c)
  IN WakeLoop(s, ws, 0, Len(SelectSeq(s.obj[c].q, Visible)), s.obj[c].senders = 0, {})

Unpark(s, w, res) ==
  [s EXCEPT !.parked = SelectSeq(@, LAMBDA u : u # w), !.lq = Append(@, w),
            !.tk[w].wait = NoWait, !.tk[w].st = "run",
            !.polls = Append(@, [t |-> w, res |-> res])]

(* poll: whether parked w's wait is over, performing its effect if so.     *)
PollOne(s, w) ==
  LET wt == s.tk[w].wait IN
  IF wt.k = "recv" THEN
    LET c == HObj(wt.h)
        q == s.obj[c].q
    IN IF s.tk[w].canceled THEN Unpark(s, w, "canceled")
       ELSE IF q # <<>> /\ Visible(q[1])
         THEN LET m == q[1]
                  s1 == [s EXCEPT !.obj[c].q = Tail(@),
                                  !.tk[w].held = @ \cup (IF m.d = "" THEN {} ELSE {m.d}),
                                  !.hist[c] = Append(@, Ev(w, wt.o, "recv", Str(m.v)))]
              IN Unpark(s1, w, Str(m.v))
       ELSE IF s.obj[c].senders = 0 THEN Unpark(s, w, "closed")
       ELSE s
  ELSE IF wt.k = "host" THEN
    IF s.tk[w].canceled
      THEN Unpark([s EXCEPT !.out = Append(@, "canceled " \o wt.h)], w, "canceled")
    ELSE IF HostReady(s, w)
      THEN Unpark([s EXCEPT !.out = Append(@, "done " \o wt.h)], w, "done")
    ELSE s
  ELSE s

RECURSIVE PollSeq(_, _)
PollSeq(s, ws) == IF ws = <<>> THEN s ELSE PollSeq(PollOne(s, Head(ws)), Tail(ws))

(* poll_parked: one readiness point.                                       *)
PollParked(s) ==
  LET s1 == [s EXCEPT !.tick = @ + 1, !.sps = @ + 1]
      woke == s1.woken \cup {w \in Range(s1.parked) : HostReady(s1, w)}
                       \cup UNION {WakeRecv(s1, c) : c \in s1.touched}
      order == SelectSeq(s1.parked, LAMBDA w : w \in woke)
  IN PollSeq([s1 EXCEPT !.touched = {}, !.woken = {}], order)

(* A scheduling point ends the turn: record it with its decisions.         *)
Record(s) ==
  [s EXCEPT !.turns = Append(@, [ops |-> s.cur, polls |-> s.polls, head |-> s.head,
                                 lq |-> s.lq, sps |-> s.sps, stop |-> s.stop]),
            !.cur = <<>>, !.polls = <<>>]

DoStop(s, kind) == Record([s EXCEPT !.stop = kind, !.head = 0])

CanComplete(s, w) == s.tk[w].wait.k = "host" /\ Cfg.delay[s.tk[w].wait.h] >= 0

(* pick, with the host wait loop and CONC-8 when nothing can run.          *)
RECURSIVE Pick(_)
Pick(s) ==
  IF s.lq # <<>> THEN Record([s EXCEPT !.head = Head(s.lq), !.lq = Tail(s.lq)])
  ELSE IF \E w \in Range(s.parked) : CanComplete(s, w) THEN Pick(PollParked(s))
  ELSE DoStop(s, IF \A t \in Tasks : s.tk[t].st = "done" THEN "done" ELSE "blocked")

SpYield(s, h) ==
  LET s1 == PollParked(s) IN
  IF s1.lq # <<>>
    THEN LET q == Append(s1.lq, h) IN Record([s1 EXCEPT !.head = Head(q), !.lq = Tail(q)])
    ELSE Record([s1 EXCEPT !.head = h])

RegisterPark(s, h) ==
  [s EXCEPT !.tk[h].st = "parked", !.tk[h].wait.at = s.parks, !.parks = @ + 1,
            !.parked = Append(@, h), !.woken = @ \cup {h}]

(* A task's end: a call returns to its caller with no scheduling point.    *)
Finish(s, h, failed) ==
  LET sibs == {u \in Tasks : u # h /\ Cfg.parent[u] = Cfg.parent[h]}
      s1 == [IF failed THEN CancelAll(s, sibs) ELSE s EXCEPT !.tk[h].st = "done"]
      w == s.tk[h].handoff
  IN IF w # 0 THEN [s1 EXCEPT !.tk[w].st = "run", !.head = w, !.tk[h].handoff = 0]
     ELSE Pick(PollParked(s1))

TurnEnd(s, h, tr) ==
  CASE tr = "yield" -> SpYield(s, h)
    [] tr = "yieldpark" -> SpYield([s EXCEPT !.tk[h].parkNext = TRUE], h)
    [] tr = "park" -> Pick(PollParked(RegisterPark(s, h)))
    [] tr = "hold" -> Pick(PollParked([s EXCEPT !.tk[h].st = "holding"]))
    [] tr = "fail" -> Finish(s, h, TRUE)
    [] OTHER -> s   \* keep, or call (ContinueCall runs the members)

-----------------------------------------------------------------------------
(* Effects of ops on run objects (commit). `early`: an early commit of     *)
(* refinement 2, whose message stays pending until its turn retires        *)
(* (rule 4). Returns the state, the outcome, the transition, the storage   *)
(* credit and the finalizer ops the dropper runs next.                     *)

Res(s, out, tr) == [s |-> s, out |-> out, tr |-> tr, cr |-> 0, fins |-> <<>>]
H(s, x, ev) == [s EXCEPT !.hist[x] = Append(@, ev)]

Eff(s0, t, o, op, early) ==
  LET s == IF op.h # "" /\ op.h \notin s0.tk[t].held
             THEN [s0 EXCEPT !.fault = "task " \o Str(t) \o " used unheld " \o op.h]
             ELSE s0
      x == IF op.h # "" THEN HObj(op.h) ELSE ""
      ob == s.obj[x]
  IN
  CASE op.k = "store" ->
         Res(H([s EXCEPT !.obj[x].val = op.v], x, Ev(t, o, "store", Str(op.v))), "", "yield")
    [] op.k = "load" ->
         Res(H(s, x, Ev(t, o, "load", Str(ob.val))), Str(ob.val), "yield")
    [] op.k = "send" ->
         IF ~ob.rx THEN Res(H(s, x, Ev(t, o, "send", "closed")), "closed", "yield")
         ELSE LET pend == early /\ "nopending" \notin Mut
                  m == [v |-> op.v, d |-> op.d, pend |-> pend, from |-> t, o |-> o]
                  s1 == [s EXCEPT !.obj[x].q = Append(@, m),
                                  !.touched = IF pend /\ "touchatcommit" \notin Mut THEN @
                                              ELSE @ \cup {x}]
              IN Res(H(s1, x, Ev(t, o, "send", Str(op.v))), "ok", "yield")
    [] op.k = "recv" ->
         IF s.tk[t].canceled THEN Res(s, "canceled", "yield")
         ELSE IF ob.q # <<>> /\ Visible(ob.q[1])
           THEN LET m == ob.q[1]
                    s1 == [s EXCEPT !.obj[x].q = Tail(@),
                                    !.tk[t].held = @ \cup (IF m.d = "" THEN {} ELSE {m.d})]
                IN Res(H(s1, x, Ev(t, o, "recv", Str(m.v))), Str(m.v), "yield")
         ELSE IF ob.senders = 0 THEN Res(s, "closed", "yield")
         ELSE Res([s EXCEPT !.tk[t].wait = [NoWait EXCEPT !.k = "recv", !.h = op.h, !.o = o]],
                  "wait", "yieldpark")
    [] op.k = "close" ->
         Res(H([s EXCEPT !.obj[x].senders = @ - 1, !.tk[t].held = @ \ {op.h},
                         !.touched = @ \cup {x}], x, Ev(t, o, "close", "")), "", "keep")
    [] op.k = "droprecv" ->
         LET left == ob.q
             s1 == H([s EXCEPT !.obj[x].rx = FALSE, !.obj[x].q = <<>>,
                               !.tk[t].held = (@ \ {op.h}) \cup
                                              {left[i].d : i \in {j \in 1 .. Len(left) : left[j].d # ""}}],
                     x, Ev(t, o, "drop", Str(Len(left))))
         IN [s |-> s1, out |-> Str(Len(left)), tr |-> "keep",
             cr |-> IF Trivial(x) THEN Len(left) * Cfg.msgsz[x] ELSE 0,
             fins |-> IF Trivial(x) THEN <<>> ELSE [i \in 1 .. Len(left) |-> Cfg.fin[x]]]
    [] op.k = "take" ->
         IF ob.holder # 0
           THEN Res([s EXCEPT !.fault = "take of a held mutex " \o x], "", "keep")
           ELSE Res(H([s EXCEPT !.obj[x].holder = t], x, Ev(t, o, "take", Str(ob.val))),
                    Str(ob.val), "keep")
    [] op.k = "put" ->
         Res(H([s EXCEPT !.obj[x].holder = 0, !.obj[x].val = op.v], x,
               Ev(t, o, "put", Str(op.v))), "", "yield")
    [] op.k = "thaw" ->
         LET out == IF ob.count = 1 THEN "ok" ELSE "err"
         IN Res(H(s, x, Ev(t, o, "thaw", out)), out, "keep")
    [] op.k = "release" ->
         [s |-> H([s EXCEPT !.obj[x].count = @ - 1, !.tk[t].held = @ \ {op.h}], x,
                  Ev(t, o, "release", "")),
          out |-> "", tr |-> "keep", cr |-> IF ob.count = 1 THEN Cfg.rootsz ELSE 0,
          fins |-> <<>>]
    [] op.k = "cancel" -> Res(CancelTree(s, op.t), "", "yield")
    [] op.k = "check" -> Res(s, IF s.tk[t].canceled THEN "canceled" ELSE "ok", "yield")
    [] op.k = "host" ->
         Res([s EXCEPT !.out = Append(@, "request " \o op.h),
                       !.tk[t].wait = [NoWait EXCEPT !.k = "host", !.h = op.h, !.o = o,
                                                     !.issued = s.tick]], "", "park")
    [] op.k = "print" -> Res([s EXCEPT !.out = Append(@, Str(op.v))], "", "yield")
    [] op.k = "fail" -> Res(s, "", "fail")
    [] op.k = "detach" ->
         Res([s EXCEPT !.tk[t].calls = <<op.t>>, !.tk[t].callYield = TRUE], "", "call")
    [] op.k = "detachall" ->
         Res([s EXCEPT !.tk[t].calls = Cfg.members[t], !.tk[t].callYield = TRUE], "", "call")
    [] op.k = "hold" ->
         IF s.tk[t].detached THEN Res(s, "", "keep")
         ELSE Res([s EXCEPT !.tk[t].wait = [NoWait EXCEPT !.k = "hold"]], "", "hold")

-----------------------------------------------------------------------------
(* Budgets (section 6, budgets.limits@2). The head charges live against    *)
(* the settled level; a non-head segment keeps pieces with an envelope of  *)
(* record highs (u, h), settled at retirement in program order.            *)

LiveCharge(s, op) ==
  CASE op.k = "alloc" ->
         IF s.level + op.v + (IF "headseesspec" \in Mut THEN s.spec ELSE 0) > s.memlim
           THEN DoStop(s, "memory") ELSE [s EXCEPT !.level = @ + op.v]
    [] op.k = "free" -> [s EXCEPT !.level = @ - op.v]
    [] op.k = "fuel" ->
         IF s.fuel + op.v > s.fuellim THEN DoStop(s, "fuel") ELSE [s EXCEPT !.fuel = @ + op.v]
    [] OTHER -> s

MaxH(env) == IF env = <<>> THEN 0 ELSE Last(env)[2]

AddCharge(p, op) ==
  CASE op.k = "alloc" ->
         LET n2 == p.n + op.v IN
         [p EXCEPT !.n = n2,
                   !.env = IF n2 <= MaxH(p.env) THEN @
                           ELSE IF @ # <<>> /\ Last(@)[1] = p.f
                             THEN Append(SubSeq(@, 1, Len(@) - 1), <<p.f, n2>>)
                             ELSE Append(@, <<p.f, n2>>)]
    [] op.k = "free" -> [p EXCEPT !.n = @ - op.v]
    [] op.k = "fuel" -> [p EXCEPT !.f = @ + op.v]
    [] OTHER -> p

(* Settlement of one piece at level l with phi fuel left.                  *)
Settle(s, p) ==
  LET phi == s.fuellim - s.fuel
      hits == {i \in 1 .. Len(p.env) : s.level + p.env[i][2] > s.memlim}
      mem == CASE "peakonly" \in Mut -> hits # {}
               [] "netonly" \in Mut -> s.level + p.n > s.memlim
               [] OTHER -> hits # {} /\ p.env[Min(hits)][1] <= phi
  IN IF mem THEN DoStop(s, "memory")
     ELSE IF p.f > phi THEN DoStop(s, "fuel")
     ELSE [s EXCEPT !.level = @ + p.n, !.fuel = @ + p.f, !.spec = @ - p.n]

(* Sequential composition of two pieces (used only by "mergepieces").      *)
Compose(p1, p2) ==
  [f |-> p1.f + p2.f, n |-> p1.n + p2.n,
   env |-> p1.env \o SelectSeq([i \in 1 .. Len(p2.env) |->
                                  <<p1.f + p2.env[i][1], p1.n + p2.env[i][2]>>],
                               LAMBDA e : e[2] > MaxH(p1.env))]

RECURSIVE ComposeAll(_)
ComposeAll(ps) == IF ps = <<>> THEN EmptyPiece ELSE Compose(Head(ps), ComposeAll(Tail(ps)))

-----------------------------------------------------------------------------
(* The head. *)

InsertFins(s, t, at, fins) == [s EXCEPT !.tk[t].prog = Insert(@, at, fins)]

ExecHead(s, h, op) ==
  IF IsCharge(op) THEN LiveCharge([s EXCEPT !.tk[h].pc = @ + 1], op)
  ELSE LET o == s.tk[h].ord + 1
           pc == s.tk[h].pc
           r == Eff([s EXCEPT !.tk[h].ord = o], h, o, op, FALSE)
           s1 == InsertFins([r.s EXCEPT !.tk[h].pc = @ + 1, !.level = @ - r.cr,
                                        !.cur = Append(@, Ev(h, o, op.k, r.out))],
                            h, pc + 1, r.fins)
       IN TurnEnd(s1, h, r.tr)

(* Retirement step 3: the message a committed-early send left pending      *)
(* becomes visible, and its channel is touched for the scheduling point.   *)
ClearPending(s, h, o, c) ==
  [s EXCEPT !.obj[c].q = [i \in 1 .. Len(@) |->
                            IF @[i].from = h /\ @[i].o = o THEN [@[i] EXCEPT !.pend = FALSE]
                            ELSE @[i]],
            !.touched = IF Mut \cap {"nopending", "touchatcommit"} # {} THEN @ ELSE @ \cup {c}]

(* The entries of the log up to and including the first that ends a turn. *)
TurnPart(log) ==
  LET ends == {i \in 1 .. Len(log) : log[i].e = "op" /\ Ends(log[i].tr)}
  IN IF ends = {} THEN log ELSE SubSeq(log, 1, Min(ends))

IsPiece(e) == e.e = "piece"
NotPiece(e) == e.e # "piece"

(* The head absorbs the next entry of its log: commits a logged op, takes  *)
(* over an early-committed one, or settles a budget piece.                 *)
Absorb(s, h) ==
  LET log == s.tk[h].log
      e == Head(log)
      turn == TurnPart(log)
  IN
  IF "mergepieces" \in Mut /\ Len(SelectSeq(turn, IsPiece)) > 0 /\ ~IsPiece(e) THEN
    \* Mutation: the turn's whole stretch is settled before any of its ops.
    LET ps == [i \in 1 .. Len(SelectSeq(turn, IsPiece)) |-> SelectSeq(turn, IsPiece)[i].p]
        rest == SelectSeq(turn, NotPiece) \o SubSeq(log, Len(turn) + 1, Len(log))
    IN Settle([s EXCEPT !.tk[h].log = rest], ComposeAll(ps))
  ELSE
  LET s0 == [s EXCEPT !.tk[h].log = Tail(@)] IN
  IF IsPiece(e) THEN Settle(s0, e.p)
  ELSE IF e.committed THEN
    LET s1 == IF e.op.k = "send" THEN ClearPending(s0, h, e.o, HObj(e.op.h)) ELSE s0
    IN TurnEnd([s1 EXCEPT !.cur = Append(@, Ev(h, e.o, e.op.k, e.out))], h, e.tr)
  ELSE
    LET r == Eff(s0, h, e.o, e.op, FALSE)
        s1 == InsertFins([r.s EXCEPT !.level = @ - r.cr,
                                     !.cur = Append(@, Ev(h, e.o, e.op.k, r.out)),
                                     !.tk[h].callLogged = e.op.k \in {"detach", "detachall"}],
                         h, s0.tk[h].pc, r.fins)
    IN TurnEnd(s1, h, r.tr)

(* A call: each member in turn; a holding one's rest runs at once (hand_off) *)
(* and its end returns here. Then the op's yield.                          *)
ContinueCall(s, h) ==
  LET t == s.tk[h] IN
  IF t.calls # <<>> THEN
    LET m == Head(t.calls)
        s1 == [s EXCEPT !.tk[h].calls = Tail(@), !.tk[m].detached = TRUE]
    IN IF s1.tk[m].st # "holding" THEN s1
       ELSE IF "callnoinsert" \in Mut /\ t.callLogged
         THEN [s1 EXCEPT !.tk[m].st = "run", !.tk[m].wait = NoWait, !.lq = Append(@, m)]
       ELSE [s1 EXCEPT !.tk[h].st = "callwait", !.tk[m].st = "run", !.tk[m].wait = NoWait,
                       !.tk[m].handoff = h, !.head = m]
  ELSE SpYield([s EXCEPT !.tk[h].callYield = FALSE, !.tk[h].callLogged = FALSE], h)

HeadStep(s) ==
  LET h == s.head
      t == s.tk[h]
  IN
  IF t.parkNext THEN Pick(PollParked(RegisterPark([s EXCEPT !.tk[h].parkNext = FALSE], h)))
  ELSE IF t.callYield THEN ContinueCall(s, h)
  ELSE IF t.log # <<>> THEN Absorb(s, h)
  ELSE IF t.pc > Len(t.prog) THEN Finish([s EXCEPT !.cur = Append(@, Ev(h, 0, "end", ""))], h, FALSE)
  ELSE ExecHead([s EXCEPT !.tk[h].pend = FALSE], h, t.prog[t.pc])

Running(s) == s.stop = "none" /\ s.fault = ""

-----------------------------------------------------------------------------
(* The width-1 reference: the head step alone, to the end of the run.      *)

RECURSIVE RunSerial(_, _)
RunSerial(s, n) == IF ~Running(s) \/ n = 0 THEN s ELSE RunSerial(HeadStep(s), n - 1)

(* With the scenario's budgets (turns, outputs, stop) and without them     *)
(* (object histories: early commits past a budget stop are internal).      *)
SB == [p \in Cfg.progs |-> RunSerial(InitState(p, Cfg.memlim, Cfg.fuellim), 400)]
SF == [p \in Cfg.progs |-> RunSerial(InitState(p, Big, Big), 400)]

ASSUME \A p \in Cfg.progs : SB[p].stop # "none" /\ SB[p].fault = ""
                             /\ SF[p].stop # "none" /\ SF[p].fault = ""

-----------------------------------------------------------------------------
(* The other workers: run-ahead and early commit.                          *)

(* Rule 3's possible footprint: the objects named by handles u holds, the  *)
(* type closure of what it can still obtain through them, and the possible *)
(* footprints of its holding children, whose rests a call of u inserts.    *)
HeldObjs(s, u) == {HObj(h) : h \in s.tk[u].held}
                    \cup (IF "noclosure" \in Mut THEN {}
                          ELSE UNION {Cfg.carries[HObj(h)] : h \in s.tk[u].held})
HoldingChildren(s, u) == {c \in Children(u) : s.tk[c].st = "holding"}
RECURSIVE PossFp(_, _)
PossFp(s, u) == HeldObjs(s, u)
                  \cup (IF "nocallterm" \in Mut THEN {}
                        ELSE UNION {PossFp(s, c) : c \in HoldingChildren(s, u)})
RECURSIVE Callers(_, _)
Callers(s, u) == IF s.tk[u].handoff = 0 THEN {}
                 ELSE {s.tk[u].handoff} \cup Callers(s, s.tk[u].handoff)
Fp(s, u) == UNION {PossFp(s, w) : w \in {u} \cup Callers(s, u)}

(* The scheduling point's term: parked receivers' channels. Rule 4 filters *)
(* these reads, so they conflict with no E op; "nofilter" removes rule 4.  *)
SpTerm(s) == IF "nofilter" \in Mut
               THEN {HObj(s.tk[w].wait.h) : w \in {u \in Range(s.parked) : s.tk[u].wait.k = "recv"}}
               ELSE {}

(* The unresolved turns before queued t's turn: the head's (with its       *)
(* callers) and those of the tasks ahead of t in the logical queue.        *)
Before(s, t) == {s.head} \cup {s.lq[i] : i \in 1 .. IndexOf(s.lq, t) - 1}

Eligible(s, t, op) ==
  LET log == s.tk[t].log IN
  /\ op.k \in EKinds
  \* Rule 1: t is queued, and op is in its queued turn.
  /\ "norule1" \in Mut \/ \A i \in 1 .. Len(log) : ~(log[i].e = "op" /\ Ends(log[i].tr))
  \* Rule 2: every earlier op of the turn is committed or disjoint from op.
  /\ "norule2" \in Mut \/ \A i \in 1 .. Len(log) :
        log[i].e = "piece" \/ log[i].committed \/ Items(log[i].op) \cap Items(op) = {}
  \* Rule 3: op commutes with every unresolved turn before t's.
  /\ "norule3" \in Mut \/ \A r \in Before(s, t) : Items(op) \cap (Fp(s, r) \cup SpTerm(s)) = {}

OpEntries(log) == Len(SelectSeq(log, NotPiece))
StretchFuel(log) == LET ps == SelectSeq(log, IsPiece) IN
                    IF ps = <<>> THEN 0 ELSE ComposeAll([i \in 1 .. Len(ps) |-> ps[i].p]).f

CanAhead(s, t) ==
  LET k == s.tk[t] IN
  /\ t \in Range(s.lq)
  /\ k.st = "run" /\ ~k.pend /\ ~k.parkNext /\ ~k.callYield
  /\ k.pc <= Len(k.prog)
  /\ OpEntries(k.log) < Cfg.runahead

Entry(op, o, committed, out, tr) ==
  [e |-> "op", op |-> op, o |-> o, committed |-> committed, out |-> out, tr |-> tr,
   p |-> EmptyPiece]

ChargeAhead(s, t, op) ==
  LET log == s.tk[t].log
      new == IF log # <<>> /\ IsPiece(Last(log))
               THEN [log EXCEPT ![Len(log)].p = AddCharge(@, op)]
               ELSE Append(log, [Entry(NoOp, 0, FALSE, "", "") EXCEPT !.e = "piece",
                                                                      !.p = AddCharge(EmptyPiece, op)])
      dspec == CASE op.k = "alloc" -> op.v [] op.k = "free" -> - op.v [] OTHER -> 0
  IN [s EXCEPT !.tk[t].log = new, !.tk[t].pc = @ + 1, !.spec = @ + dspec]

(* A non-head segment pauses (unobservably) at an allocation beyond the    *)
(* speculation allowance, or when its own fuel exceeds the fuel left.      *)
Paused(s, t, op) ==
  \/ op.k = "alloc" /\ s.spec + op.v > Cfg.allowance
  \/ op.k = "fuel" /\ StretchFuel(s.tk[t].log) + op.v > s.fuellim - s.fuel

EarlyCommit(s, t, op, o) ==
  LET r == Eff([s EXCEPT !.tk[t].ord = o], t, o, op, TRUE)
      s1 == [r.s EXCEPT !.tk[t].pc = @ + 1,
                        !.tk[t].log = Append(@, Entry(op, o, TRUE, r.out, r.tr)),
                        !.early = @ \cup {<<t, o>>}]
  IN [ok |-> r.tr # "yieldpark",
      s |-> IF "earlysp" \in Mut /\ r.tr = "yield" THEN PollParked(s1) ELSE s1]

LogOp(s, t, op, o) ==
  IF "immrelease" \in Mut /\ op.k = "release"
    THEN LET r == Eff([s EXCEPT !.tk[t].ord = o], t, o, op, TRUE)
         IN [r.s EXCEPT !.tk[t].pc = @ + 1,
                        !.tk[t].log = Append(@, Entry(op, o, TRUE, r.out, r.tr))]
    ELSE [s EXCEPT !.tk[t].ord = o, !.tk[t].pc = @ + 1,
                   !.tk[t].log = Append(@, Entry(op, o, FALSE, "", StaticTr(op)))]

AheadSucc(s, t) ==
  LET k == s.tk[t]
      op == k.prog[k.pc]
      o == k.ord + 1
  IN
  IF IsCharge(op) THEN (IF Paused(s, t, op) THEN {} ELSE {ChargeAhead(s, t, op)})
  ELSE LET ec == EarlyCommit(s, t, op, o)
           early == IF Eligible(s, t, op) /\ ec.ok THEN {ec.s} ELSE {}
           other == IF IsD(op) THEN {LogOp(s, t, op, o)} ELSE {[s EXCEPT !.tk[t].pend = TRUE]}
       IN IF Eager /\ early # {} THEN early ELSE early \cup other

-----------------------------------------------------------------------------
(* The specification. *)

VARIABLE st

Init == \E p \in Cfg.progs : st = InitState(p, Cfg.memlim, Cfg.fuellim)

AheadAny(s) == UNION {IF CanAhead(s, t) THEN AheadSucc(s, t) ELSE {} : t \in Tasks}

Next ==
  \/ /\ Running(st)
     /\ \/ /\ ~Eager \/ AheadAny(st) = {}
           /\ st' = HeadStep(st)
        \/ st' \in AheadAny(st)
  \/ ~Running(st) /\ UNCHANGED st

Spec == Init /\ [][Next]_st

-----------------------------------------------------------------------------
(* Invariants. *)

Ref == SB[st.p]
Full == SF[st.p]

NoFault == st.fault = ""

(* (a) The retired turns are the width-1 run's first k turns, with their   *)
(* ops, outcomes, polls, readiness-point counts and scheduling decisions;  *)
(* the turn in progress is a prefix of the next one.                       *)
TurnsPrefix ==
  /\ Len(st.turns) <= Len(Ref.turns)
  /\ \A i \in 1 .. Len(st.turns) : st.turns[i] = Ref.turns[i]
  /\ Len(st.turns) < Len(Ref.turns) => IsPrefix(st.cur, Ref.turns[Len(st.turns) + 1].ops)
  /\ IsPrefix(st.out, Ref.out)

(* (b) Every object's committed sub-events are a prefix of its width-1     *)
(* sequence, each with its width-1 outcome; the pending messages are       *)
(* exactly those of senders whose turn has not retired.                    *)
HistPrefix == \A x \in Objects : IsPrefix(st.hist[x], Full.hist[x])

InLog(t, o) == \E i \in 1 .. Len(st.tk[t].log) :
                 st.tk[t].log[i].e = "op" /\ st.tk[t].log[i].o = o /\ st.tk[t].log[i].committed

PendingExact ==
  \A c \in Objects : \A i \in 1 .. Len(st.obj[c].q) :
    LET m == st.obj[c].q[i] IN m.from # 0 => (m.pend <=> InLog(m.from, m.o))

(* (c) The logical queue equals the width-1 queue after the same           *)
(* scheduling points.                                                      *)
QueueEqual ==
  IF st.turns = <<>> THEN st.lq = Tail(Cfg.queue)
  ELSE st.lq = st.turns[Len(st.turns)].lq

(* At the end, the run is the width-1 run.                                 *)
FinalEqual ==
  ~Running(st) =>
    /\ st.turns = Ref.turns /\ st.out = Ref.out /\ st.stop = Ref.stop
    /\ st.stop \notin {"memory", "fuel"} => st.hist = Full.hist

(* What a run exposes: mutation runs must fail on this, not on bookkeeping. *)
Behavior == NoFault /\ TurnsPrefix /\ HistPrefix /\ QueueEqual /\ FinalEqual

Soundness == Behavior /\ PendingExact

(* Witnesses: the generic runs must reach an early commit and a logged op *)
(* (check.sh expects these to be violated).                                *)
NoEarlyCommit == st.early = {}
NoLoggedOp == \A t \in Tasks : \A i \in 1 .. Len(st.tk[t].log) :
                st.tk[t].log[i].e = "op" => st.tk[t].log[i].committed

(* Positive traces: under Eager, by the time the first turn retires, the   *)
(* expected ops have committed early.                                      *)
Positive == Len(st.turns) >= 1 => Cfg.expect \subseteq st.early

=============================================================================
