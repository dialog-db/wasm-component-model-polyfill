// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

// The demo's service worker. A service worker must add its listeners
// while this script is first evaluated, and it can neither `await` at
// the top level nor `import()` a module. So the script loads the
// worker's wasm-bindgen output as a classic script, starts the Rust
// module without waiting for it, and adds every listener at once. Each
// event waits until the start completes and then calls into Rust.
//
// Every request goes to Rust, because `respondWith` must be called
// while the event dispatches, before the script could ask Rust whether
// a route claims the request. Rust sends a request that no route claims
// to the network itself.

importScripts("./wcmp-demo-worker.js");

const started = wasm_bindgen({ module_or_path: "./wcmp-demo-worker_bg.wasm" });

self.addEventListener("install", (event) => {
  event.waitUntil(self.skipWaiting());
});

self.addEventListener("activate", (event) => {
  event.waitUntil(self.clients.claim());
});

// A module that failed to start answers nothing, so every request then
// goes to the network, and the page still loads.
self.addEventListener("fetch", (event) => {
  event.respondWith(
    started.then(
      () => wasm_bindgen.handle_fetch(event.request),
      () => fetch(event.request),
    ),
  );
});

// A page that no worker controls, as after a hard reload, asks for a
// claim. The script answers it itself, so it works whatever the module
// does.
self.addEventListener("message", (event) => {
  if (event.data?.kind === "claim") {
    event.waitUntil(self.clients.claim());
    return;
  }
  event.waitUntil(
    started.then(
      () => wasm_bindgen.handle_message(event.data, event.ports[0]),
      (error) => event.ports[0]?.postMessage({ failure: String(error) }),
    ),
  );
});
