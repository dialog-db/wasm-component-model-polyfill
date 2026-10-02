;; Copyright 2026 The Dialog DB Project
;;
;; This Source Code Form is subject to the terms of the Mozilla Public
;; License, v. 2.0. If a copy of the MPL was not distributed with this
;; file, You can obtain one at https://mozilla.org/MPL/2.0/.

;; The core module behind the `socket` world: imports `math`, exports `run`.
(module
  (import "wcmp:fixtures/math" "double" (func $double (param i32) (result i32)))
  (func (export "run") (param i32) (result i32)
    local.get 0 call $double i32.const 1 i32.add))
