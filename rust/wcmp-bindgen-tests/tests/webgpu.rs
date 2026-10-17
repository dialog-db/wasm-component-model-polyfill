// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `wasi:webgpu` generates: the WIT of `wasi:webgpu@0.3.0-rc.2`
//! (`wit-webgpu/`, `WebAssembly/wasi-webgpu` at commit `6a776ba`), with
//! a `with` mapping of its 33 resources to types of a host core and the
//! `store` flag on `lost` and on `on-uncaptured-error`, the two
//! functions that return a future or a stream. Every other function
//! that waits on the GPU is an `async func`, a method of a resource.
//!
//! The test builds for both targets, so the output compiles on each.
//! The impl below of `gpu-device`'s store-side trait is what a host
//! writes; without the `store` flag on `lost` it does not compile,
//! which the crate's documentation shows with a doctest that fails to
//! build where the impl names `Access`.

#![cfg(test)]

use wcmp::{Access, Accessor, Error, FutureReader, HasData, Resource, StreamReader};

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// The host core: one type for each resource of `wasi:webgpu`.
pub mod core {
    pub struct Gpu;
    pub struct GpuAdapter;
    pub struct GpuAdapterInfo;
    pub struct GpuBindGroup;
    pub struct GpuBindGroupLayout;
    pub struct GpuBuffer;
    pub struct GpuCanvasContext;
    pub struct GpuCommandBuffer;
    pub struct GpuCommandEncoder;
    pub struct GpuCompilationInfo;
    pub struct GpuCompilationMessage;
    pub struct GpuComputePassEncoder;
    pub struct GpuComputePipeline;
    pub struct GpuDevice;
    pub struct GpuDeviceLostInfo;
    pub struct GpuError;
    pub struct GpuPipelineLayout;
    pub struct GpuQuerySet;
    pub struct GpuQueue;
    pub struct GpuRenderBundle;
    pub struct GpuRenderBundleEncoder;
    pub struct GpuRenderPassEncoder;
    pub struct GpuRenderPipeline;
    pub struct GpuSampler;
    pub struct GpuShaderModule;
    pub struct GpuSupportedFeatures;
    pub struct GpuSupportedLimits;
    pub struct GpuTexture;
    pub struct GpuTextureView;
    pub struct GpuUncapturedErrorEvent;
    pub struct RecordGpuPipelineConstantValue;
    pub struct RecordOptionGpuSize64;
    pub struct WgslLanguageFeatures;
}

mod webgpu {
    wcmp::bindgen!({
        path: "wit-webgpu",
        world: "imports",
        imports: {
            "wasi:webgpu/webgpu.[method]gpu-device.lost": store | trappable,
            "wasi:webgpu/webgpu.[method]gpu-device.on-uncaptured-error": store | trappable,
            default: trappable,
        },
        with: {
            "wasi:webgpu/webgpu.gpu": crate::core::Gpu,
            "wasi:webgpu/webgpu.gpu-adapter": crate::core::GpuAdapter,
            "wasi:webgpu/webgpu.gpu-adapter-info": crate::core::GpuAdapterInfo,
            "wasi:webgpu/webgpu.gpu-bind-group": crate::core::GpuBindGroup,
            "wasi:webgpu/webgpu.gpu-bind-group-layout": crate::core::GpuBindGroupLayout,
            "wasi:webgpu/webgpu.gpu-buffer": crate::core::GpuBuffer,
            "wasi:webgpu/webgpu.gpu-canvas-context": crate::core::GpuCanvasContext,
            "wasi:webgpu/webgpu.gpu-command-buffer": crate::core::GpuCommandBuffer,
            "wasi:webgpu/webgpu.gpu-command-encoder": crate::core::GpuCommandEncoder,
            "wasi:webgpu/webgpu.gpu-compilation-info": crate::core::GpuCompilationInfo,
            "wasi:webgpu/webgpu.gpu-compilation-message": crate::core::GpuCompilationMessage,
            "wasi:webgpu/webgpu.gpu-compute-pass-encoder": crate::core::GpuComputePassEncoder,
            "wasi:webgpu/webgpu.gpu-compute-pipeline": crate::core::GpuComputePipeline,
            "wasi:webgpu/webgpu.gpu-device": crate::core::GpuDevice,
            "wasi:webgpu/webgpu.gpu-device-lost-info": crate::core::GpuDeviceLostInfo,
            "wasi:webgpu/webgpu.gpu-error": crate::core::GpuError,
            "wasi:webgpu/webgpu.gpu-pipeline-layout": crate::core::GpuPipelineLayout,
            "wasi:webgpu/webgpu.gpu-query-set": crate::core::GpuQuerySet,
            "wasi:webgpu/webgpu.gpu-queue": crate::core::GpuQueue,
            "wasi:webgpu/webgpu.gpu-render-bundle": crate::core::GpuRenderBundle,
            "wasi:webgpu/webgpu.gpu-render-bundle-encoder": crate::core::GpuRenderBundleEncoder,
            "wasi:webgpu/webgpu.gpu-render-pass-encoder": crate::core::GpuRenderPassEncoder,
            "wasi:webgpu/webgpu.gpu-render-pipeline": crate::core::GpuRenderPipeline,
            "wasi:webgpu/webgpu.gpu-sampler": crate::core::GpuSampler,
            "wasi:webgpu/webgpu.gpu-shader-module": crate::core::GpuShaderModule,
            "wasi:webgpu/webgpu.gpu-supported-features": crate::core::GpuSupportedFeatures,
            "wasi:webgpu/webgpu.gpu-supported-limits": crate::core::GpuSupportedLimits,
            "wasi:webgpu/webgpu.gpu-texture": crate::core::GpuTexture,
            "wasi:webgpu/webgpu.gpu-texture-view": crate::core::GpuTextureView,
            "wasi:webgpu/webgpu.gpu-uncaptured-error-event": crate::core::GpuUncapturedErrorEvent,
            "wasi:webgpu/webgpu.record-gpu-pipeline-constant-value": crate::core::RecordGpuPipelineConstantValue,
            "wasi:webgpu/webgpu.record-option-gpu-size64": crate::core::RecordOptionGpuSize64,
            "wasi:webgpu/webgpu.wgsl-language-features": crate::core::WgslLanguageFeatures,
        },
    });
}

use webgpu::wasi::webgpu::webgpu::{
    CreatePipelineError, GpuComputePipelineDescriptor, GpuRenderPipelineDescriptor,
    HostGpuDeviceWithStore, PopErrorScopeError,
};

/// The host's `HasData`.
pub struct HasGpu;

impl HasData for HasGpu {
    type Data<'a> = ();
}

/// The error a stub returns where a host would reach the GPU.
fn unsupported(what: &str) -> Error {
    Error::Unsupported {
        feature: format!("`{what}` in the test's stub host"),
    }
}

impl<T: 'static> HostGpuDeviceWithStore<T> for HasGpu {
    fn lost(
        _host: Access<'_, T, Self>,
        _self_: Resource<core::GpuDevice>,
    ) -> wcmp::Result<FutureReader<Resource<core::GpuDeviceLostInfo>>> {
        Err(unsupported("lost"))
    }

    fn on_uncaptured_error(
        _host: Access<'_, T, Self>,
        _self_: Resource<core::GpuDevice>,
    ) -> wcmp::Result<StreamReader<Resource<core::GpuError>>> {
        Err(unsupported("on-uncaptured-error"))
    }

    async fn create_compute_pipeline_async(
        _accessor: &Accessor<T, Self>,
        _self_: Resource<core::GpuDevice>,
        _descriptor: GpuComputePipelineDescriptor,
    ) -> wcmp::Result<Result<Resource<core::GpuComputePipeline>, CreatePipelineError>> {
        Err(unsupported("create-compute-pipeline-async"))
    }

    async fn create_render_pipeline_async(
        _accessor: &Accessor<T, Self>,
        _self_: Resource<core::GpuDevice>,
        _descriptor: GpuRenderPipelineDescriptor,
    ) -> wcmp::Result<Result<Resource<core::GpuRenderPipeline>, CreatePipelineError>> {
        Err(unsupported("create-render-pipeline-async"))
    }

    async fn pop_error_scope(
        _accessor: &Accessor<T, Self>,
        _self_: Resource<core::GpuDevice>,
    ) -> wcmp::Result<Result<Option<Resource<core::GpuError>>, PopErrorScopeError>> {
        Err(unsupported("pop-error-scope"))
    }
}

/// Whether `D` serves `gpu-device`'s store-side functions for any host
/// data.
fn serves<D: HostGpuDeviceWithStore<()>>() -> bool {
    true
}

#[wcmp_macros::test]
fn it_generates_wasi_webgpu_with_its_resources_mapped() {
    // The mapped resource is the host core's own type.
    let _: Resource<webgpu::wasi::webgpu::webgpu::GpuBuffer> =
        Resource::<core::GpuBuffer>::new_own(1);
    assert!(
        serves::<HasGpu>(),
        "the host's impl of the store-side trait compiles"
    );
}
