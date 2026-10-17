// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The name of what a projection of the host data gives.

/// The name of what a projection of the host data gives, as
/// Wasmtime's `HasData` is.
///
/// A host crate that serves an interface implements the interface's
/// traits on a view of its own state, not on the embedder's whole host
/// data, whose type it does not know. A type that implements this
/// trait names that view: [`Data`](Self::Data) is the view, for the
/// length of one borrow of the host data. The embedder registers the
/// crate with a projection, a function `fn(&mut T) -> D::Data<'_>` that
/// reaches the view from its host data `T`.
///
/// No value of a `HasData` type exists. The type only names the
/// result of the projection, so a unit struct or an empty enum does.
/// A view can hold several separate borrows of the host data:
///
/// ```rust
/// use wcmp::{HasData, ResourceTable};
///
/// struct GpuCtx;
///
/// struct GpuView<'a> {
///     ctx: &'a mut GpuCtx,
///     table: &'a mut ResourceTable,
/// }
///
/// struct HasGpu;
///
/// impl HasData for HasGpu {
///     type Data<'a> = GpuView<'a>;
/// }
///
/// struct Host {
///     gpu: GpuCtx,
///     table: ResourceTable,
/// }
///
/// let project: fn(&mut Host) -> GpuView<'_> = |host| GpuView {
///     ctx: &mut host.gpu,
///     table: &mut host.table,
/// };
/// # let _ = project;
/// ```
///
/// [`HasSelf`](crate::HasSelf) is the projection to the whole host
/// data.
pub trait HasData: 'static {
    /// What the projection gives, for the length of one borrow of the
    /// host data.
    type Data<'a>;
}
