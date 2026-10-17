// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The tests of `wcmp::bindgen!`, under `tests/`.
//!
//! The crate has no code of its own. It is the crate the tests invoke the
//! macro from, so the macro reads `wit/` beside its manifest when an
//! invocation names no path, and every relative path in an invocation
//! resolves against its directory.
//!
//! The `store` flag on `wasi:webgpu`'s `lost` is what puts it in
//! `HostGpuDeviceWithStore<T>` with an `Access`. Without the flag, a
//! host's impl of `lost` there does not compile: `lost` is a method of
//! `HostGpuDevice`, which takes `&mut self` and no `Access`.
//!
//! ```compile_fail,E0407
//! mod webgpu {
//!     wcmp::bindgen!({
//!         path: "wit-webgpu",
//!         world: "imports",
//!         imports: { default: trappable },
//!     });
//! }
//! use webgpu::wasi::webgpu::webgpu::*;
//! use wcmp::{Access, Accessor, FutureReader, HasData, Resource};
//!
//! struct HasGpu;
//!
//! impl HasData for HasGpu {
//!     type Data<'a> = ();
//! }
//!
//! impl<T: 'static> HostGpuDeviceWithStore<T> for HasGpu {
//!     fn lost(
//!         _host: Access<'_, T, Self>,
//!         _self_: Resource<GpuDevice>,
//!     ) -> wcmp::Result<FutureReader<Resource<GpuDeviceLostInfo>>> {
//!         unimplemented!()
//!     }
//!
//!     async fn create_compute_pipeline_async(
//!         _accessor: &Accessor<T, Self>,
//!         _self_: Resource<GpuDevice>,
//!         _descriptor: GpuComputePipelineDescriptor,
//!     ) -> wcmp::Result<Result<Resource<GpuComputePipeline>, CreatePipelineError>> {
//!         unimplemented!()
//!     }
//!
//!     async fn create_render_pipeline_async(
//!         _accessor: &Accessor<T, Self>,
//!         _self_: Resource<GpuDevice>,
//!         _descriptor: GpuRenderPipelineDescriptor,
//!     ) -> wcmp::Result<Result<Resource<GpuRenderPipeline>, CreatePipelineError>> {
//!         unimplemented!()
//!     }
//!
//!     async fn pop_error_scope(
//!         _accessor: &Accessor<T, Self>,
//!         _self_: Resource<GpuDevice>,
//!     ) -> wcmp::Result<Result<Option<Resource<GpuError>>, PopErrorScopeError>> {
//!         unimplemented!()
//!     }
//! }
//! ```
