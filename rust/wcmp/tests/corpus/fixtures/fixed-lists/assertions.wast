;; `wast` has no syntax for a fixed-length list value: the harness
;; spells one as a list where the declared type is `list<T, N>`.
(assert_return
  (invoke "sum" (list.const (u32.const 1) (u32.const 2) (u32.const 3) (u32.const 36)))
  (u32.const 42))
(assert_return
  (invoke "double" (list.const (u8.const 0) (u8.const 13) (u8.const 26) (u8.const 39) (u8.const 52) (u8.const 65) (u8.const 78) (u8.const 91) (u8.const 104) (u8.const 117) (u8.const 130) (u8.const 143) (u8.const 156) (u8.const 169) (u8.const 182) (u8.const 200)))
  (list.const (u8.const 0) (u8.const 26) (u8.const 52) (u8.const 78) (u8.const 104) (u8.const 130) (u8.const 156) (u8.const 182) (u8.const 208) (u8.const 234) (u8.const 4) (u8.const 30) (u8.const 56) (u8.const 82) (u8.const 108) (u8.const 144)))
(assert_return
  (invoke "identity" (list.const (u8.const 0) (u8.const 13) (u8.const 26) (u8.const 39) (u8.const 52) (u8.const 65) (u8.const 78) (u8.const 91) (u8.const 104) (u8.const 117) (u8.const 130) (u8.const 143) (u8.const 156) (u8.const 169) (u8.const 182) (u8.const 200)))
  (list.const (u8.const 0) (u8.const 13) (u8.const 26) (u8.const 39) (u8.const 52) (u8.const 65) (u8.const 78) (u8.const 91) (u8.const 104) (u8.const 117) (u8.const 130) (u8.const 143) (u8.const 156) (u8.const 169) (u8.const 182) (u8.const 200)))
