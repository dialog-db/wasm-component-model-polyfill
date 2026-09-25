;; `total` calls `count-up` in the other component and reads the
;; `stream<u32>` it answers with, so each number crosses from the
;; counter's memory into the summer's. The summer traps on a number out
;; of order and on a stream that ends early, so a sum comes back only
;; when every number arrived once, in order.
(assert_return (invoke "total" (u32.const 1000)) (u64.const 500500))
(assert_return (invoke "total" (u32.const 1)) (u64.const 1))
(assert_return (invoke "total" (u32.const 0)) (u64.const 0))
