;; This fixture is the `wasi-http` fixture as first written, kept as a
;; tripwire on the spec. Its `drain` resolves a
;; `future<result<_, error-code>>` whose two ends one instance holds,
;; and the Component Model traps such a copy, as a temporary rule, when
;; the payload is not a number type. Once the spec lifts that rule the
;; directives below can pass with no change to the file.
;;
;; The component imports `wasi:http/types@0.3.0`, and the harness
;; registers no host for it, so the component definition above fails
;; at link and every directive after it cascades before `drain` runs.
;; The repository test `baseline_wasi_http_handler` supplies that
;; interface and checks the trap.
;;
;; `wasi:http/handler@0.3.0` lives inside an exported instance, and a
;; `request` is a resource. The harness resolves an `invoke` against
;; root-level exports, as Wasmtime's wast runner does, and `wast` has
;; no syntax for a resource value, so no directive can hand the
;; handler a request. `drain` carries the same body-and-trailers
;; machinery in a signature a directive can call: it writes its bytes
;; into a `stream<u8>`, reads them back, and resolves a `future`
;; beside them.
(assert_return
  (invoke "drain" (list.const (u8.const 104) (u8.const 105)))
  (list.const (u8.const 104) (u8.const 105)))
(assert_return (invoke "drain" (list.const)) (list.const))
