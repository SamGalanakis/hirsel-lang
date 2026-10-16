import Rc.Sticky

/-!
# Reachable states

Every state the evaluator (`run`, behind `rc-trace`) reaches from the empty state satisfies
the invariant, so every theorem stated for `Inv` applies to every vector's final state.
The published theorem set is listed in `Audit.lean` and `README.md`.
-/

namespace Rc

theorem run_inv {cfg : Config} (hok : cfg.Ok) :
    ∀ (ops : List Op) (s s' : State) (i : Nat), Inv cfg s → run cfg s ops i = .ok s' →
      Inv cfg s'
  | [], s, s', _, hi, h => by simp [run] at h; subst h; exact hi
  | op :: ops, s, s', i, hi, h => by
    simp only [run] at h
    split at h
    · cases h
    · rename_i s1 h1
      exact run_inv hok ops s1 s' (i + 1) (step_inv hok hi h1) h

/-- **Every reachable state is safe**: running any trace from the empty state, every state
reached satisfies the invariant. -/
theorem reachable_inv {cfg : Config} (hok : cfg.Ok) {ops : List Op} {s' : State}
    (h : run cfg {} ops 0 = .ok s') : Inv cfg s' :=
  run_inv hok ops {} s' 0 inv_init h

end Rc
