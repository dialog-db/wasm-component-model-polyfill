//! The backend contract of the runtime layer, on the Wasmtime backend.

#![cfg(not(target_arch = "wasm32"))]

use wcmp_wasm_core::Engine;
use wcmp_wasm_core_wasmtime::Wasmtime;

fn engine() -> Engine {
    Engine::with_backend(Wasmtime::new().expect("Wasmtime makes an engine"))
}

wcmp_wasm_core_contract::contract_tests!(engine);
