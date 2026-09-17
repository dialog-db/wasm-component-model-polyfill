//! Where a completed host task leaves what its future produced.

use std::sync::{Arc, Mutex};

use crate::error::Result;
use crate::value::Val;

/// Where a completed host task leaves what its future produced.
///
/// The slot is filled once, by the turn that saw the future
/// complete. The crossing from this value into the guest's memory is
/// the boundary context's work, and the caller that started the host
/// task owns the slot, so the scheduler only fills it.
pub type HostTaskResult = Arc<Mutex<Option<Result<Vec<Val>>>>>;
