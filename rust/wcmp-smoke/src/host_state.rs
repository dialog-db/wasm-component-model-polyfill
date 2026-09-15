/// What the host records while the guests run. Every step's store
/// carries one of these, so a host function and a destructor have a
/// place to leave evidence the step reads back.
#[derive(Debug, Default)]
pub struct HostState {
    /// Values guest code handed to the `tally` host function.
    pub tallies: Vec<u32>,
    /// Representations of host resources the guest dropped, in order.
    pub dropped: Vec<u32>,
}
