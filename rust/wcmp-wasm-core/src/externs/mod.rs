//! The handles to the objects of a store, and the externs among them.

mod external;
mod func;
mod global;
mod instance;
mod memory;
mod table;
mod tag;

pub use external::Extern;
pub use func::Func;
pub use global::Global;
pub use instance::Instance;
pub use memory::Memory;
pub use table::Table;
pub use tag::Tag;
