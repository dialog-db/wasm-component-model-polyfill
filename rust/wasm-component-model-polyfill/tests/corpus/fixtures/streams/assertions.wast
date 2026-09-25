;; Both exports take or return a stream or a future, and `wast` has no
;; syntax for either, so no directive can call them. The definition
;; directive above checks that the polyfill translates and
;; instantiates the component. The smoke test (`rust/wcmp-smoke`)
;; calls both exports from a host that reads and writes the streams.
