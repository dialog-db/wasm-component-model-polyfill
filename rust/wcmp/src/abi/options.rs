//! The canonical-ABI options one boundary crossing runs under.
//!
//! A `canon lift` or `canon lower` declares its options as indexes
//! into the runtime slabs the executor's `Extract*` directives fill:
//! a memory, a `cabi_realloc`, a `post-return`, a callback, a string
//! encoding, and a data model. [`BoundaryOptions`] is that bundle
//! once the slots are read, so the crossing holds the memory and the
//! functions themselves rather than the indexes that name them.
//!
//! Resolving the slots is the reason this type lives under
//! [`crate::abi`]: the runtime-layer memory and the guest's
//! `cabi_realloc` are named here and nowhere else, and a call site
//! hands the lift and lower code the canon options the translator
//! recorded, or the memory slot of an adapter's transcoder.

use std::sync::{Arc, Mutex};

use crate::abi::runtime_state::AbiRuntimeState;
use crate::concurrency::InstanceId;
use crate::error::{Error, Result};
use crate::executor::ir::{CanonOptions, DataModel, StringEncoding};
use crate::internal::ErrorInternal;
use crate::runtime_layer::{Func as RuntimeFunc, Memory};

/// The canonical-ABI options of one crossing, resolved against the
/// instance's runtime state.
#[derive(Clone)]
pub struct BoundaryOptions {
    /// The options the translator recorded, when a `canon`
    /// definition declared them. A copy between two guest memories,
    /// which an adapter's transcoder performs, declares none: the
    /// fused adapter names the two memories and nothing else.
    ///
    /// Nothing reads them back yet. A `task.return` compares its own
    /// against the lift options of its task, and both are these.
    #[allow(dead_code)]
    declared: Option<Arc<CanonOptions>>,
    /// The store-wide identity of the component instance the
    /// declared options belong to.
    instance: Option<InstanceId>,
    /// The guest memory the values of the crossing sit in.
    memory: Option<Memory>,
    /// The guest's `cabi_realloc`, which every heap-allocating lower
    /// needs.
    realloc: Option<RuntimeFunc>,
    /// The export's `post-return`, run once the caller has observed
    /// the return value.
    post_return: Option<RuntimeFunc>,
    /// The export's callback, resumed once per event the task of an
    /// asynchronous call receives. Only a lift that declared the
    /// `async` option has one.
    callback: Option<RuntimeFunc>,
    /// The encoding a `string`-typed value crosses in.
    string_encoding: StringEncoding,
    /// Where the values of the crossing live.
    data_model: DataModel,
}

impl BoundaryOptions {
    /// Resolve the canon options `declared` against a runtime state
    /// the caller has already locked: the memory, `cabi_realloc`,
    /// `post-return`, and callback slots the `Extract*` directives
    /// filled, and the identity of the component instance the
    /// options name.
    ///
    /// The lock is the caller's because a call site needs the
    /// instance's tables along with the options, and both come out of
    /// the same state: [`BoundaryInstance::resolve`] takes the lock
    /// once and reads the pair through here.
    ///
    /// [`BoundaryInstance::resolve`]:
    ///     crate::abi::instance::BoundaryInstance::resolve
    pub fn from_state(declared: &Arc<CanonOptions>, state: &AbiRuntimeState) -> Self {
        Self {
            declared: Some(Arc::clone(declared)),
            instance: state.component_instances.get(declared.instance).copied(),
            memory: declared
                .memory
                .and_then(|slot| state.memories.get(slot).and_then(|m| m.clone())),
            realloc: declared
                .realloc
                .and_then(|slot| state.reallocs.get(slot).and_then(|f| f.clone())),
            post_return: declared
                .post_return
                .and_then(|slot| state.post_returns.get(slot).and_then(|f| f.clone())),
            callback: declared
                .callback
                .and_then(|slot| state.callbacks.get(slot).and_then(|f| f.clone())),
            string_encoding: declared.string_encoding,
            data_model: declared.data_model,
        }
    }

    /// The options of one side of a copy between two guest memories.
    /// An adapter's transcoder names a memory slot and the encodings
    /// of the conversion, so these options carry the memory alone.
    /// The slot must be filled: a transcoder runs only after the
    /// `ExtractMemory` directive for the memory it addresses.
    pub fn for_memory(slot: usize, abi_state: &Arc<Mutex<AbiRuntimeState>>) -> Result<Self> {
        let state = abi_state
            .lock()
            .map_err(|_| Error::internal("ABI runtime state lock poisoned"))?;
        let memory = state
            .memories
            .get(slot)
            .and_then(|m| m.clone())
            .ok_or_else(|| {
                Error::internal("an adapter addressed a memory that is not extracted")
            })?;
        Ok(Self {
            declared: None,
            instance: None,
            memory: Some(memory),
            realloc: None,
            post_return: None,
            callback: None,
            string_encoding: StringEncoding::Utf8,
            data_model: DataModel::LinearMemory,
        })
    }

    /// The canon options the translator recorded, when the crossing
    /// is one a `canon` definition declared. A `task.return`
    /// compares its own against these; nothing else reads them.
    #[allow(dead_code)]
    pub fn declared(&self) -> Option<&CanonOptions> {
        self.declared.as_deref()
    }

    /// The component instance the declared options belong to.
    pub fn instance(&self) -> Option<InstanceId> {
        self.instance
    }

    /// The guest memory the values of the crossing sit in.
    pub fn memory(&self) -> Option<&Memory> {
        self.memory.as_ref()
    }

    /// The guest's `cabi_realloc`.
    pub fn realloc(&self) -> Option<&RuntimeFunc> {
        self.realloc.as_ref()
    }

    /// The export's `post-return`.
    pub fn post_return(&self) -> Option<&RuntimeFunc> {
        self.post_return.as_ref()
    }

    /// The export's callback. A host call into an asynchronous export
    /// reads it here and hands it to the callback task, which calls it
    /// each time the task resumes.
    pub fn callback(&self) -> Option<&RuntimeFunc> {
        self.callback.as_ref()
    }

    /// The encoding a `string`-typed value crosses in.
    pub fn string_encoding(&self) -> StringEncoding {
        self.string_encoding
    }

    /// Where the values of the crossing live.
    pub fn data_model(&self) -> DataModel {
        self.data_model
    }
}
