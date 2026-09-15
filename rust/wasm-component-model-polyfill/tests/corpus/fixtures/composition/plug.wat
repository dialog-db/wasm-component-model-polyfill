;; The core module behind the `plug` world: exports the `math` interface.
(module
  (func (export "wcmp:fixtures/math#double") (param i32) (result i32)
    local.get 0 i32.const 2 i32.mul))
