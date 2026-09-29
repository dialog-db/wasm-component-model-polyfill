//! Whether a global can change.

/// Whether a global can be set after it is made.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mutability {
    /// The global holds one value for its life.
    Const,
    /// The global can be set.
    Var,
}
