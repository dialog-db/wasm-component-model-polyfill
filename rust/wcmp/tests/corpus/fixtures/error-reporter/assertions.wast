;; `describe` takes an `error-context`, and `wast` has no syntax for
;; one, so no directive can call it. The definition directive above
;; checks that the polyfill translates and instantiates the component
;; with the gate on. The smoke test (`rust/wcmp-smoke`) hands
;; `describe` an error context another component returned to the host.
