;; `wast` has no syntax for a map value: the harness spells one as a
;; list of two-element tuples where the declared type is a map.
(assert_return
  (invoke "sum" (list.const
    (tuple.const (str.const "a") (u32.const 1))
    (tuple.const (str.const "b") (u32.const 2))
    (tuple.const (str.const "c") (u32.const 39))))
  (u32.const 42))
(assert_return (invoke "sum" (list.const)) (u32.const 0))
(assert_return
  (invoke "keys" (list.const
    (tuple.const (str.const "first") (u32.const 1))
    (tuple.const (str.const "second") (u32.const 2))))
  (list.const (str.const "first") (str.const "second")))
(assert_return
  (invoke "make" (list.const
    (tuple.const (str.const "x") (u32.const 7))
    (tuple.const (str.const "y") (u32.const 8))))
  (list.const
    (tuple.const (str.const "x") (u32.const 7))
    (tuple.const (str.const "y") (u32.const 8))))
(assert_return
  (invoke "identity" (list.const
    (tuple.const (str.const "hello") (u32.const 4294967295))))
  (list.const
    (tuple.const (str.const "hello") (u32.const 4294967295))))
(assert_return (invoke "identity" (list.const)) (list.const))
