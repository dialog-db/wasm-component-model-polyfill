;; The component imports `wasi:http/types@0.3.0`, and the harness
;; registers no host for it, so the component definition above fails
;; at link and every directive after it cascades. The polyfill runs
;; the handler and `drain` once a host supplies that interface, as the
;; repository test `baseline_wasi_http_handler` does.
;;
;; `wasi:http/handler@0.3.0` lives inside an exported instance, and a
;; `request` is a resource. The harness resolves an `invoke` against
;; root-level exports, as Wasmtime's wast runner does, and `wast` has
;; no syntax for a resource value, so no directive can hand the
;; handler a request. `drain` carries the same body-and-trailers
;; machinery in a signature a directive can call: it writes its bytes
;; into a `stream<u8>`, reads them back, and resolves a `future<u32>`
;; with the count beside them.
(assert_return
  (invoke "drain" (list.const (u8.const 104) (u8.const 105)))
  (list.const (u8.const 104) (u8.const 105)))
(assert_return (invoke "drain" (list.const)) (list.const))
