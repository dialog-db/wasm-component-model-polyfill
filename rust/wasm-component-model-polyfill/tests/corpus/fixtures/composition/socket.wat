;; The core module behind the `socket` world: imports `math`, exports `run`.
(module
  (import "wcmp:fixtures/math" "double" (func $double (param i32) (result i32)))
  (func (export "run") (param i32) (result i32)
    local.get 0 call $double i32.const 1 i32.add))
