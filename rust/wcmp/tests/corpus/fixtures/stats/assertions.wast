;; `sum` and `average` answer for a list of values. `average` of an
;; empty list divides by zero, which the guest's Rust turns into a
;; panic and the release profile into an `unreachable` trap. The trap
;; poisons the store, so a later call of `sum` in the same store is
;; refused with the cannot-enter trap. The smoke test
;; (`rust/wcmp-smoke`) tells the same story and then builds a new
;; store to call `sum` again.
(assert_return (invoke "sum" (list.const (u32.const 1) (u32.const 2) (u32.const 39))) (u32.const 42))
(assert_return (invoke "average" (list.const (u32.const 2) (u32.const 4))) (u32.const 3))
(assert_trap (invoke "average" (list.const)) "unreachable")
(assert_trap (invoke "sum" (list.const (u32.const 1))) "cannot enter component instance")
