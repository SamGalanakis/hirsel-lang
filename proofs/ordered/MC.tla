--------------------------------- MODULE MC ---------------------------------
(***************************************************************************)
(* Scenarios for Ordered.tla: the named traces of design section 4.3, the  *)
(* generic 3 tasks x 4 ops mix, and the scenarios the mutation runs use.   *)
(* check.sh pairs each with its constants and invariants.                  *)
(*                                                                         *)
(* Tasks are 1..3. The first task of `queue` is the head when the modeled  *)
(* window opens; the rest are queued in order. Handles name objects:       *)
(* atomics x y z a2 a3, mutex m, channels c (senders s1 s2 s3, receiver    *)
(* cr) and d (sender sd, receiver dr), root r (holders r1 r2 r3) and the   *)
(* host key of request h. A channel's sender and receiver counts and a     *)
(* root's count follow from who holds which handle.                        *)
(***************************************************************************)
EXTENDS Ordered

Hobj == [x |-> "x", y |-> "y", z |-> "z", a2 |-> "a2", a3 |-> "a3", m |-> "m",
         s1 |-> "c", s2 |-> "c", s3 |-> "c", cr |-> "c", sd |-> "d", dr |-> "d",
         r1 |-> "r", r2 |-> "r", r3 |-> "r", h |-> "hostkey"]
DataObjs == {"x", "y", "z", "a2", "a3", "m", "c", "d", "r"}
AllObjs == DataObjs \cup {"hostkey"}

Msg(v) == [v |-> v, d |-> "", pend |-> FALSE, from |-> 0, o |-> 0]
RecvWait(h) == [NoWait EXCEPT !.k = "recv", !.h = h]
HostWait(h) == [NoWait EXCEPT !.k = "host", !.h = h]
HoldWait == [NoWait EXCEPT !.k = "hold"]

Base ==
  [n |-> 3, progs |-> {},
   held |-> <<{}, {}, {}>>, parent |-> <<0, 0, 0>>, members |-> <<<<>>, <<>>, <<>>>>,
   queue |-> <<1, 2, 3>>, st0 |-> <<"run", "run", "run">>,
   wait0 |-> <<NoWait, NoWait, NoWait>>, parked0 |-> <<>>,
   hobj |-> Hobj, senders |-> {"s1", "s2", "s3", "sd"},
   q0 |-> [x \in DataObjs |-> <<>>], carries |-> [x \in AllObjs |-> {}],
   fin |-> [x \in DataObjs |-> NoOp], msgsz |-> [x \in DataObjs |-> 0],
   delay |-> [h \in {"h"} |-> -1],
   memlim |-> Big, fuellim |-> Big, level0 |-> 0, allowance |-> Big,
   runahead |-> 4, rootsz |-> 0, expect |-> {}]

-----------------------------------------------------------------------------
(* Named traces of section 4.3 (Model first).                              *)

(* A Sender close (keep, logged) followed by an atomic store, against a    *)
(* runnable load: the store stays in the closer's turn, so 3 loads 1.      *)
CloseStoreLoad ==
  [Base EXCEPT
     !.progs = {<<<<Work, Work, Store("y", 1)>>,
                  <<Close("s2"), Store("x", 1)>>,
                  <<Load("x")>>>>},
     !.held = <<{"y", "cr"}, {"s2", "x"}, {"x"}>>]

(* A Receiver drop whose finalizer yields: the leftover's finalizer        *)
(* (a store, which yields) runs in the dropper's turn, before 3's load.    *)
DropFinalizerYields ==
  [Base EXCEPT
     !.progs = {<<<<Work, Store("y", 1)>>,
                  <<DropRecv("cr"), Store("x", 1)>>,
                  <<Load("z"), Load("x")>>>>},
     !.held = <<{"y"}, {"cr", "z", "x"}, {"z", "x"}>>,
     !.q0 = [@ EXCEPT !["c"] = <<Msg(5)>>],
     !.fin = [@ EXCEPT !["c"] = Store("z", 7)]]

(* detach_all over two holding members whose cleanups yield: the parent    *)
(* logs it ahead; at retirement each member's rest runs as a call, in      *)
(* member order, and returns before the parent's next op.                  *)
DetachAllYields ==
  [Base EXCEPT
     !.progs = {<<<<DetachAll, Emit(1)>>,
                  <<Hold, Store("a2", 1)>>,
                  <<Hold, Store("a3", 1)>>>>},
     !.held = <<{}, {"a2"}, {"a3"}>>,
     !.parent = <<0, 1, 1>>, !.members = <<<<2, 3>>, <<>>, <<>>>>,
     !.queue = <<2, 3, 1>>]

(* A head that detaches a holding child whose finalizer yields, while a    *)
(* queued sibling has already committed an independent early send.         *)
DetachWhileEarlySend ==
  [Base EXCEPT
     !.progs = {<<<<Detach(2), Emit(1)>>,
                  <<Store("a2", 1)>>,
                  <<Send("s3", 5), Recv("cr")>>>>},
     !.held = <<{}, {"a2"}, {"s3", "cr"}>>,
     !.parent = <<0, 1, 0>>,
     !.st0 = <<"run", "holding", "run">>, !.wait0 = <<NoWait, HoldWait, NoWait>>,
     !.queue = <<1, 3>>,
     !.expect = {<<3, 1>>}]

(* The same shape where the child's rest writes what the sibling reads:    *)
(* rule 3's call term (the holding child's footprint) refuses the load.    *)
DetachCallTerm ==
  [DetachWhileEarlySend EXCEPT
     !.progs = {<<<<Detach(2), Emit(1)>>,
                  <<Store("x", 1)>>,
                  <<Load("x")>>>>},
     !.held = <<{}, {"x"}, {"x"}>>,
     !.expect = {}]

(* Readiness-point counts against width 1: a host wait, a receive that     *)
(* parks after its yield, and an early store and send around them.         *)
ReadinessCounts ==
  [Base EXCEPT
     !.progs = {<<<<HostReq("h"), Emit(1)>>,
                  <<Recv("cr"), Emit(2)>>,
                  <<Store("x", 1), Send("s3", 7)>>>>},
     !.held = <<{"h"}, {"cr"}, {"x", "s3"}>>,
     !.delay = [@ EXCEPT !["h"] = 2]]

(* An early send to a parked receiver stays pending through the polls      *)
(* before its turn and wakes the receiver at its turn's scheduling point.  *)
EarlySendParked ==
  [Base EXCEPT
     !.progs = {<<<<Work, Store("y", 1)>>,
                  <<Emit(2)>>,
                  <<Send("s3", 7)>>>>},
     !.held = <<{"y"}, {"cr"}, {"s3"}>>,
     !.st0 = <<"run", "parked", "run">>, !.wait0 = <<NoWait, RecvWait("cr"), NoWait>>,
     !.parked0 = <<2>>, !.queue = <<1, 3>>,
     !.expect = {<<3, 1>>}]

(* An older head's poll of a parked host waiter while a younger parent     *)
(* logs task.cancel of that waiter: the cancel commits only at retirement. *)
CancelHostWaiter ==
  [Base EXCEPT
     !.progs = {<<<<Work, Store("y", 1)>>,
                  <<Emit(2)>>,
                  <<CancelT(2), Emit(3)>>>>},
     !.held = <<{"y"}, {"h"}, {}>>,
     !.parent = <<0, 3, 0>>,
     !.st0 = <<"run", "parked", "run">>, !.wait0 = <<NoWait, HostWait("h"), NoWait>>,
     !.parked0 = <<2>>, !.queue = <<1, 3>>]

(* A sibling's failure against an early store (passes) and a receive       *)
(* (never early: it observes the cancellation).                            *)
FailureStoreRecv ==
  [Base EXCEPT
     !.progs = {<<<<Work, Fail>>,
                  <<Store("x", 1)>>,
                  <<Recv("cr")>>>>},
     !.held = <<{}, {"x"}, {"cr"}>>,
     !.q0 = [@ EXCEPT !["c"] = <<Msg(9)>>],
     !.expect = {<<2, 1>>}]

(* thaw against a younger release: the release is deferred to its turn.   *)
ThawRelease ==
  [Base EXCEPT
     !.progs = {<<<<Work, Thaw("r1")>>,
                  <<Release("r2")>>,
                  <<Store("x", 1)>>>>},
     !.held = <<{"r1"}, {"r2"}, {"x"}>>]

(* A mutex body reaching thaw: the early taker waits at the head-only      *)
(* thaw holding m; the head still retires, and the next taker is refused.  *)
MutexBodyThaw ==
  [Base EXCEPT
     !.progs = {<<<<Work, Store("y", 1)>>,
                  <<Take("m"), Thaw("r2"), Put("m", 1)>>,
                  <<Take("m"), Put("m", 2)>>>>},
     !.held = <<{"y"}, {"m", "r2"}, {"m"}>>,
     !.expect = {<<2, 1>>}]

(* Review-2 finding 3, (1): the live head against settled 0 while a        *)
(* younger segment holds 80 inside the speculation allowance. No stop.     *)
Budget2a ==
  [Base EXCEPT
     !.progs = {<<<<Alloc(80), Free(80), Store("y", 1)>>,
                  <<Alloc(80), Store("x", 1)>>,
                  <<>>>>},
     !.held = <<{"y"}, {"x"}, {}>>,
     !.memlim = 100]

(* Review-2 finding 3, (2): a younger summary whose record high 20 stops   *)
(* the run at its turn, 90 + 20 > 100, although its net is 0.              *)
Budget2b ==
  [Base EXCEPT
     !.progs = {<<<<Alloc(90), Store("y", 1)>>,
                  <<Alloc(20), Free(20), Store("x", 1)>>,
                  <<>>>>},
     !.held = <<{"y"}, {"x"}, {}>>,
     !.memlim = 100]

(* Review-3 finding 3, first example: settled 90, limit 100, one fuel      *)
(* point left. "allocate 20; 2 fuel; free 20" stops with kind memory ...   *)
Budget3a ==
  [Base EXCEPT
     !.progs = {<<<<Fuel(5), Store("y", 1)>>,
                  <<Alloc(20), Fuel(2), Free(20), Store("x", 1)>>,
                  <<>>>>},
     !.held = <<{"y"}, {"x"}, {}>>,
     !.memlim = 100, !.level0 = 90, !.fuellim = 6]

(* ... and "2 fuel; allocate 20; free 20" with kind fuel.                  *)
Budget3b ==
  [Budget3a EXCEPT
     !.progs = {<<<<Fuel(5), Store("y", 1)>>,
                  <<Fuel(2), Alloc(20), Free(20), Store("x", 1)>>,
                  <<>>>>}]

(* Review-3 finding 3, second example: a close of a trivially droppable    *)
(* Receiver frees 80 before a 20-byte allocation: 90 - 80 + 20 = 30.       *)
Budget3c ==
  [Base EXCEPT
     !.progs = {<<<<Work, Store("y", 1)>>,
                  <<DropRecv("cr"), Alloc(20), Store("x", 1)>>,
                  <<>>>>},
     !.held = <<{"y"}, {"cr", "x"}, {}>>,
     !.q0 = [@ EXCEPT !["c"] = <<Msg(1)>>], !.msgsz = [@ EXCEPT !["c"] = 80],
     !.memlim = 100, !.level0 = 90]

(* Positive: two siblings' mutex and atomic ops pass a long head.          *)
PassMutexAtomic ==
  [Base EXCEPT
     !.progs = {<<<<Work, Work, Store("y", 1)>>,
                  <<Take("m"), Put("m", 1)>>,
                  <<Store("x", 1)>>>>},
     !.held = <<{"y"}, {"m"}, {"x"}>>,
     !.expect = {<<2, 1>>, <<2, 2>>, <<3, 1>>}]

(* Positive: two siblings' send and atomic load pass a long head.          *)
PassSendLoad ==
  [Base EXCEPT
     !.progs = {<<<<Work, Work, Store("y", 1)>>,
                  <<Send("s2", 1)>>,
                  <<Load("x"), Recv("cr")>>>>},
     !.held = <<{"y"}, {"s2"}, {"x", "cr"}>>,
     !.expect = {<<2, 1>>, <<3, 1>>}]

-----------------------------------------------------------------------------
(* Rule scenarios (each rule's mutation run fails on one of these).        *)

(* Rule 1: a task commits early within one turn per head. Its second store *)
(* is in its next turn, after 3's load.                                    *)
OneTurnPerHead ==
  [Base EXCEPT
     !.progs = {<<<<Work, Store("y", 1)>>,
                  <<Store("x", 1), Store("x", 2)>>,
                  <<Load("x")>>>>},
     !.held = <<{"y"}, {"x"}, {"x"}>>]

(* Rule 2: a logged close of one Sender, then a send on another Sender of  *)
(* the same channel: the send must not commit before the close.            *)
LoggedCloseThenSend ==
  [Base EXCEPT
     !.progs = {<<<<Work, Store("y", 1)>>,
                  <<Close("s2"), Send("s3", 4)>>,
                  <<Recv("cr")>>>>},
     !.held = <<{"y"}, {"s2", "s3"}, {"cr"}>>]

(* Handles through channels: the head drops a Receiver whose leftover     *)
(* message carries the handle of z, and the leftover's finalizer stores to *)
(* z in the head's turn. The type closure of c (carries) puts z in the     *)
(* head's possible footprint before it holds z, so 3's load waits.         *)
HandleThroughChannel ==
  [Base EXCEPT
     !.progs = {<<<<Work, DropRecv("cr")>>,
                  <<Store("x", 1)>>,
                  <<Load("z")>>>>},
     !.held = <<{"cr"}, {"x"}, {"z"}>>,
     !.q0 = [@ EXCEPT !["c"] = <<[Msg(1) EXCEPT !.d = "z"]>>],
     !.fin = [@ EXCEPT !["c"] = Store("z", 7)],
     !.carries = [@ EXCEPT !["c"] = {"z"}]]

-----------------------------------------------------------------------------
(* The generic run: 3 tasks x 4 ops, every combination of four candidate   *)
(* programs per task, over shared atomics, a mutex, a channel and a root.  *)

Cand1 == {<<Store("x", 1), Send("s1", 1), Close("s1"), Release("r1")>>,
          <<Take("m"), Thaw("r1"), Put("m", 1), Load("x")>>,
          <<Load("x"), Close("s1"), Release("r1"), Store("x", 2)>>,
          <<Send("s1", 3), Take("m"), Put("m", 3), Close("s1")>>}
Cand2 == {<<Store("y", 1), Send("s2", 2), Close("s2"), Release("r2")>>,
          <<Take("m"), Put("m", 2), Release("r2"), Load("y")>>,
          <<Release("r2"), Load("y"), Send("s2", 4), Close("s2")>>,
          <<Send("s2", 5), Store("y", 5), Close("s2"), Check>>}
Cand3 == {<<Recv("cr"), Load("x"), Recv("cr"), Thaw("r3")>>,
          <<Load("y"), Store("x", 3), Release("r3"), Recv("cr")>>,
          <<Thaw("r3"), Recv("cr"), Store("y", 3), Load("x")>>,
          <<Store("x", 9), Store("y", 9), Load("x"), DropRecv("cr")>>}

Mix ==
  [Base EXCEPT
     !.progs = {<<p1, p2, p3>> : p1 \in Cand1, p2 \in Cand2, p3 \in Cand3},
     !.held = <<{"x", "m", "s1", "r1"}, {"y", "m", "s2", "r2"}, {"x", "y", "cr", "r3"}>>]

(* A second mix with task-tree ops: cancellation, a host wait, a failing   *)
(* child, a detach of a holding child, and a Receiver drop whose leftover  *)
(* has a finalizer.                                                        *)
TCand1 == {<<CancelT(2), Store("x", 1), Detach(3), Load("y")>>,
           <<Detach(3), Store("x", 2), Check, Emit(1)>>,
           <<Store("x", 3), Send("s1", 1), CancelT(3), Close("s1")>>}
TCand2 == {<<HostReq("h"), Load("x"), Check, Store("y", 1)>>,
           <<Store("y", 2), Check, Emit(2), Fail>>,
           <<Check, Store("y", 3), HostReq("h"), Emit(2)>>}
TCand3 == {<<Hold, Recv("cr"), Store("z", 1), Emit(3)>>,
           <<Hold, DropRecv("cr"), Load("z"), Emit(3)>>,
           <<Store("z", 2), Hold, Recv("cr"), Load("z")>>}

TreeMix ==
  [Base EXCEPT
     !.progs = {<<p1, p2, p3>> : p1 \in TCand1, p2 \in TCand2, p3 \in TCand3},
     !.held = <<{"x", "y", "s1"}, {"x", "y", "h"}, {"z", "cr"}>>,
     !.parent = <<0, 1, 1>>,
     !.q0 = [@ EXCEPT !["c"] = <<Msg(4)>>],
     !.fin = [@ EXCEPT !["c"] = Store("z", 8)],
     !.delay = [@ EXCEPT !["h"] = 1],
     !.queue = <<3, 2, 1>>]

=============================================================================
