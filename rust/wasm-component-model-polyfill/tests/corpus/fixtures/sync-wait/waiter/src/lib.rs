//! A guest whose synchronous code calls a host `async` function, built
//! by `cargo` and wit-bindgen.
//!
//! Both functions are `async func`s in the WIT, and the `async` option
//! below binds both synchronously: the import is lowered without the
//! `async` option, so a call returns only once the host has answered,
//! and the export is lifted without it, so its core function is plain
//! code on the task's one thread. A host answer that is not ready at
//! once has to hold that thread where it stands until the answer
//! arrives, which is what a stack switch does.

wit_bindgen::generate!({
    path: "../wit",
    world: "sync-wait",
    async: ["-import:host-echo-u32", "-export:total"],
});

struct Waiter;

impl Guest for Waiter {
    /// One blocking call per key, in order, summed.
    fn total(keys: Vec<u32>) -> u32 {
        keys.into_iter()
            .map(host_echo_u32)
            .fold(0, u32::wrapping_add)
    }
}

export!(Waiter);
