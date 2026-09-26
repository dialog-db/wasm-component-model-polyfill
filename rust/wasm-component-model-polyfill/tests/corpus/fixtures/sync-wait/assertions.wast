;; The harness's `host-echo-u32` answers at once, so these calls never
;; wait; the smoke test (`rust/wcmp-smoke`) supplies an answer that
;; arrives after a timer, which holds the guest's thread until then.
(assert_return (invoke "total" (list.const)) (u32.const 0))
(assert_return (invoke "total" (list.const (u32.const 1) (u32.const 2) (u32.const 39))) (u32.const 42))
