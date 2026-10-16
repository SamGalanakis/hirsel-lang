import Rc.Machine

/-! General list facts used by the proofs. -/

namespace Rc

theorem countP_erase_add {α : Type} [DecidableEq α] (p : α → Bool) :
    ∀ (l : List α) (c : α), c ∈ l → (l.erase c).countP p + (if p c then 1 else 0) = l.countP p
  | [], c, h => by simp at h
  | d :: l, c, h => by
    by_cases hdc : d = c
    · subst hdc; simp [List.countP_cons]
    · have hc : c ∈ l := by
        rcases List.mem_cons.mp h with h | h
        · exact absurd h.symm hdc
        · exact h
      have ih := countP_erase_add p l c hc
      have hbeq : (d == c) = false := by simpa using hdc
      simp only [List.erase_cons, hbeq, List.countP_cons]
      simp only [Bool.false_eq_true, ↓reduceIte, List.countP_cons]
      omega

theorem countP_erase_le {α : Type} [DecidableEq α] (p : α → Bool) (l : List α) (c : α) :
    (l.erase c).countP p ≤ l.countP p :=
  List.Sublist.countP_le (List.erase_sublist)

theorem mem_erase_of_mem_ne {α : Type} [DecidableEq α] {l : List α} {a b : α}
    (h : a ∈ l) (hne : a ≠ b) : a ∈ l.erase b :=
  (List.mem_erase_of_ne hne).mpr h

theorem nodup_map_erase {α β : Type} [DecidableEq α] (f : α → β) {l : List α} (c : α)
    (h : (l.map f).Nodup) : ((l.erase c).map f).Nodup :=
  h.sublist (List.erase_sublist.map f)

theorem find?_mem {α : Type} {p : α → Bool} {l : List α} {a : α} (h : l.find? p = some a) :
    a ∈ l := List.mem_of_find?_eq_some h

theorem find?_prop {α : Type} {p : α → Bool} {l : List α} {a : α} (h : l.find? p = some a) :
    p a = true := List.find?_some h

/-- In a list whose images under `f` are distinct, two members with the same image are equal. -/
theorem eq_of_map_nodup {α β : Type} {f : α → β} :
    ∀ {l : List α}, (l.map f).Nodup → ∀ {a b : α}, a ∈ l → b ∈ l → f a = f b → a = b
  | [], _, _, _, ha, _, _ => by simp at ha
  | x :: l, h, a, b, ha, hb, hab => by
    rw [List.map_cons, List.nodup_cons] at h
    have hx : ∀ y ∈ l, f y ≠ f x := fun y hy he => h.1 (he ▸ List.mem_map_of_mem hy)
    rcases List.mem_cons.mp ha with ha' | ha' <;> rcases List.mem_cons.mp hb with hb' | hb'
    · rw [ha', hb']
    · subst ha'; exact absurd hab.symm (hx b hb')
    · subst hb'; exact absurd hab (hx a ha')
    · exact eq_of_map_nodup h.2 ha' hb' hab

theorem nodup_of_map {α β : Type} {f : α → β} {l : List α} (h : (l.map f).Nodup) : l.Nodup :=
  List.Pairwise.of_map f (fun _ _ h e => h (congrArg f e)) h

end Rc
