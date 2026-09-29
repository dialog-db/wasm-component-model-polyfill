//! A tag.

use crate::checks;
use crate::error::Result;
use crate::internal::StoreContextInternal;
use crate::store::AsContext;
use crate::types::TagType;

handle! {
    /// A tag of exception handling or stack switching.
    ///
    /// A tag is an extern kind. The host can import a tag, export a tag,
    /// link a tag from one instance to another, and read its type. The host
    /// cannot make a tag or throw an exception.
    Tag
}

impl Tag {
    /// The type of the tag, whose parameters are its payload.
    pub fn ty(&self, store: impl AsContext) -> Result<TagType> {
        let backend = store.as_context().backend();
        checks::same_store(backend, *self)?;
        backend.tag_ty(*self)
    }
}
