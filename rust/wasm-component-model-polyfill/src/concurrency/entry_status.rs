//! Where a thread entry stopped when a provider handed control back.

use wasm_runtime_layer::Val as RuntimeVal;

/// Where a thread entry stopped when a provider's start or resume
/// handed control back to its caller.
///
/// A provider reports this at the moment the entry stops, so the
/// caller never has to ask again. The results of an entry that
/// finished come with the answer: the switch module's entry wrapper
/// handed them to the host before it returned, whether or not the
/// entry suspended on the way.
#[derive(Clone, Debug)]
pub enum EntryStatus {
    /// The entry returned these core results, and its thread is
    /// gone from the provider.
    Finished(Vec<RuntimeVal>),
    /// The entry suspended in a shim, and the provider keeps its
    /// thread until a resume names it.
    Suspended,
}
