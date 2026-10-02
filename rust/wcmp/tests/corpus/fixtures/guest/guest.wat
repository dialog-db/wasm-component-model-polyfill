;; Copyright 2026 The Dialog DB Project
;;
;; This Source Code Form is subject to the terms of the Mozilla Public
;; License, v. 2.0. If a copy of the MPL was not distributed with this
;; file, You can obtain one at https://mozilla.org/MPL/2.0/.

;; The core module behind the `guest` world: one root-level export.
(module
  (func (export "double") (param i32) (result i32)
    local.get 0 i32.const 2 i32.mul))
