;; The polyfill rejects an `async` lift today, so the component
;; definition above is an expected failure and every directive after
;; it cascades. The fixture holds the target: it flips to passing when
;; the async lift form, the stream and future built-ins, `error-context`,
;; and the task built-ins land, with no change to the file.
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
