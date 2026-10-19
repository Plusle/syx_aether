#![allow(dead_code)]

//! Private object-safe backend contracts. Creation never performs hidden submission.
//! Backend resources retain native device state independently of the public Device.

use super::command::{
    BufferCopy, BufferImageCopy, DynamicState, ImageBlit, ImageCopy, IndexBufferBinding,
    RenderingDesc, VertexBufferBinding,
};
use super::descriptor::{
    DescriptorPoolDesc, DescriptorSetLayout, DescriptorSetLayoutDesc, DescriptorWrite,
    PipelineLayout, PipelineLayoutDesc, PipelineLayoutId,
};
use super::device::{Capabilities, DeviceSelection};
use super::error::{RhiResult, ValidationMessage};
use super::lifetime::ObjectId;
use super::pipeline::{PipelineDesc, PipelineKind};
use super::query::{QueryPool, QueryPoolDesc, QueryResult, TimestampStage};
use super::resource::{Buffer, BufferDesc, Image, ImageDesc, ImageViewDesc, SamplerDesc};
use super::shader::{ShaderInterface, ShaderModuleDesc};
use super::swapchain::{
    AcquireToken, PresentOutcome, PresentToken, SurfaceCapabilities, SwapchainDesc, SwapchainId,
};
use super::sync::{DeviceId, QueueCapabilities, QueueId};
use super::tracking::PlannedBarrier;
use super::types::{BackendType, BufferRange, QueueKind, ShaderStageFlags};
use std::any::Any;
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

/// Any permits checked backend-local downcasts; no reinterpretation is part of the API.
pub(crate) trait RhiObject: Any + Send + Sync {
    fn object_id(&self) -> ObjectId;
}

pub(crate) trait RhiDevice: Any + Send + Sync {
    fn device_id(&self) -> DeviceId;
    fn selection(&self) -> &DeviceSelection;
    fn capabilities(&self) -> &Capabilities;
    fn create_queue(&self, id: QueueId, kind: QueueKind) -> RhiResult<Arc<dyn RhiQueue>>;
    fn create_buffer(&self, id: ObjectId, desc: &BufferDesc) -> RhiResult<Arc<dyn RhiBuffer>>;
    fn create_image(&self, id: ObjectId, desc: &ImageDesc) -> RhiResult<Arc<dyn RhiImage>>;
    fn create_image_view(
        &self,
        id: ObjectId,
        image: &Arc<dyn RhiImage>,
        desc: &ImageViewDesc,
    ) -> RhiResult<Arc<dyn RhiImageView>>;
    fn create_sampler(&self, id: ObjectId, desc: &SamplerDesc) -> RhiResult<Arc<dyn RhiSampler>>;
    fn create_shader_module(
        &self,
        id: ObjectId,
        desc: &ShaderModuleDesc,
    ) -> RhiResult<Arc<dyn RhiShaderModule>>;
    fn create_descriptor_set_layout(
        &self,
        id: ObjectId,
        desc: &DescriptorSetLayoutDesc,
    ) -> RhiResult<Arc<dyn RhiDescriptorSetLayout>>;
    fn create_pipeline_layout(
        &self,
        id: ObjectId,
        desc: &PipelineLayoutDesc<'_>,
    ) -> RhiResult<Arc<dyn RhiPipelineLayout>>;
    fn create_pipeline(
        &self,
        id: ObjectId,
        desc: &PipelineDesc<'_>,
    ) -> RhiResult<Arc<dyn RhiPipeline>>;
    fn create_descriptor_pool(
        &self,
        desc: &DescriptorPoolDesc,
    ) -> RhiResult<Arc<dyn RhiDescriptorPool>>;
    fn surface_capabilities(&self, surface: &dyn RhiSurface) -> RhiResult<SurfaceCapabilities>;
    fn create_swapchain(
        &self,
        id: SwapchainId,
        surface: &Arc<dyn RhiSurface>,
        desc: &SwapchainDesc,
    ) -> RhiResult<Box<dyn RhiSwapchain>>;
    fn create_query_pool(
        &self,
        id: ObjectId,
        desc: &QueryPoolDesc,
    ) -> RhiResult<Arc<dyn RhiQueryPool>>;
    fn wait(&self, queue: &dyn RhiQueue, value: u64, timeout: Option<Duration>) -> RhiResult<()>;
    /// Full-device waits are reserved for shutdown by the common layer.
    fn wait_idle(&self) -> RhiResult<()>;
    fn set_object_name(&self, object: &dyn RhiObject, name: &str) -> RhiResult<()>;
    fn drain_validation_messages(&self) -> Vec<ValidationMessage>;
    fn load_pipeline_cache(&self, bytes: &[u8]) -> RhiResult<()>;
    fn pipeline_cache_data(&self) -> RhiResult<Vec<u8>>;
}

pub(crate) struct QueueWait<'a> {
    pub(crate) queue: &'a dyn RhiQueue,
    pub(crate) value: u64,
}

pub(crate) struct PresentSubmission<'a> {
    pub(crate) acquire: &'a AcquireToken,
    pub(crate) signal: PresentToken,
}

/// The common queue allocates signal values under its shared submit gate.
/// Success schedules work only; transactions are committed by the common layer.
pub(crate) trait RhiQueue: Any + Send + Sync {
    fn queue_id(&self) -> QueueId;
    fn capabilities(&self) -> QueueCapabilities;
    fn allocate_command_buffer(&self) -> RhiResult<Box<dyn RhiCommandBuffer>>;
    fn recycle_command_buffer(&self, buffer: Box<dyn RhiCommandBuffer>) -> RhiResult<()>;
    fn submit(
        &self,
        buffer: &dyn RhiCommandBuffer,
        waits: &[QueueWait<'_>],
        signal_value: u64,
        present: Option<PresentSubmission<'_>>,
    ) -> RhiResult<()>;
    fn completed_value(&self) -> RhiResult<u64>;
    fn wait(&self, value: u64, timeout: Option<Duration>) -> RhiResult<()>;
    fn wait_idle(&self) -> RhiResult<()>;
}

/// Mutably recorded by one worker, then moved to a submitter. Sync is not required.
/// Recording methods never retain the borrowed public facade objects.
pub(crate) trait RhiCommandBuffer: Any + Send {
    fn origin(&self) -> QueueId;
    fn begin(&mut self) -> RhiResult<()>;
    fn finish(&mut self) -> RhiResult<()>;
    fn barriers(&mut self, barriers: &[PlannedBarrier<'_>]) -> RhiResult<()>;
    fn begin_rendering(&mut self, desc: &RenderingDesc<'_>) -> RhiResult<()>;
    fn end_rendering(&mut self) -> RhiResult<()>;
    fn begin_compute(&mut self) -> RhiResult<()>;
    fn end_compute(&mut self) -> RhiResult<()>;
    fn bind_pipeline(&mut self, pipeline: &dyn RhiPipeline) -> RhiResult<()>;
    fn bind_descriptor_sets(
        &mut self,
        kind: PipelineKind,
        layout: &PipelineLayout,
        first_set: u32,
        sets: &[&dyn RhiDescriptorSet],
        dynamic_offsets: &[u32],
    ) -> RhiResult<()>;
    fn bind_vertex_buffers(
        &mut self,
        first_binding: u32,
        buffers: &[VertexBufferBinding<'_>],
    ) -> RhiResult<()>;
    fn bind_index_buffer(&mut self, binding: &IndexBufferBinding<'_>) -> RhiResult<()>;
    fn push_constants(
        &mut self,
        layout: &PipelineLayout,
        stages: ShaderStageFlags,
        offset: u32,
        bytes: &[u8],
    ) -> RhiResult<()>;
    fn set_dynamic_state(&mut self, state: &DynamicState) -> RhiResult<()>;
    fn draw(&mut self, vertices: Range<u32>, instances: Range<u32>) -> RhiResult<()>;
    fn draw_indexed(
        &mut self,
        indices: Range<u32>,
        vertex_offset: i32,
        instances: Range<u32>,
    ) -> RhiResult<()>;
    fn draw_indirect(
        &mut self,
        buffer: &Buffer,
        offset: u64,
        draw_count: u32,
        stride: u32,
    ) -> RhiResult<()>;
    fn draw_indexed_indirect(
        &mut self,
        buffer: &Buffer,
        offset: u64,
        draw_count: u32,
        stride: u32,
    ) -> RhiResult<()>;
    fn dispatch(&mut self, groups: [u32; 3]) -> RhiResult<()>;
    fn dispatch_indirect(&mut self, buffer: &Buffer, offset: u64) -> RhiResult<()>;
    fn copy_buffer(&mut self, src: &Buffer, dst: &Buffer, regions: &[BufferCopy]) -> RhiResult<()>;
    fn copy_buffer_to_image(
        &mut self,
        src: &Buffer,
        dst: &Image,
        regions: &[BufferImageCopy],
    ) -> RhiResult<()>;
    fn copy_image_to_buffer(
        &mut self,
        src: &Image,
        dst: &Buffer,
        regions: &[BufferImageCopy],
    ) -> RhiResult<()>;
    fn copy_image(&mut self, src: &Image, dst: &Image, regions: &[ImageCopy]) -> RhiResult<()>;
    fn blit_image(&mut self, src: &Image, dst: &Image, regions: &[ImageBlit]) -> RhiResult<()>;
    fn update_buffer(&mut self, dst: &Buffer, offset: u64, bytes: &[u8]) -> RhiResult<()>;
    fn fill_buffer(&mut self, dst: &Buffer, range: BufferRange, value: u32) -> RhiResult<()>;
    fn reset_queries(&mut self, pool: &QueryPool, queries: Range<u32>) -> RhiResult<()>;
    fn write_timestamp(
        &mut self,
        pool: &QueryPool,
        query: u32,
        stage: TimestampStage,
    ) -> RhiResult<()>;
    fn begin_debug_label(&mut self, name: &str, color: [f32; 4]) -> RhiResult<()>;
    fn end_debug_label(&mut self) -> RhiResult<()>;
    fn insert_debug_label(&mut self, name: &str, color: [f32; 4]) -> RhiResult<()>;
}

/// Host operations must enforce bounds and memory class, flush/invalidate if needed,
/// and never wait for or submit GPU work. Common reservations protect in-flight ranges.
pub(crate) trait RhiBuffer: RhiObject {
    fn desc(&self) -> &BufferDesc;
    fn write(&self, offset: u64, bytes: &[u8]) -> RhiResult<()>;
    fn read(&self, offset: u64, len: usize) -> RhiResult<Vec<u8>>;
}

/// Ordinary image allocations and externally owned swapchain images share this contract.
/// A swapchain image's backend object must retain its native generation owner.
pub(crate) trait RhiImage: RhiObject {
    fn desc(&self) -> &ImageDesc;
    fn ownership(&self) -> ImageOwnership;
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum ImageOwnership {
    Allocated,
    Swapchain { id: SwapchainId, generation: u64 },
}

pub(crate) trait RhiImageView: RhiObject {
    fn desc(&self) -> &ImageViewDesc;
    fn image(&self) -> &Arc<dyn RhiImage>;
}

pub(crate) trait RhiSampler: RhiObject {
    fn desc(&self) -> &SamplerDesc;
}

pub(crate) trait RhiShaderModule: RhiObject {
    fn interface(&self) -> &ShaderInterface;
}

pub(crate) trait RhiDescriptorSetLayout: RhiObject {
    fn desc(&self) -> &DescriptorSetLayoutDesc;
}

pub(crate) trait RhiPipelineLayout: RhiObject {
    fn set_layouts(&self) -> &[Arc<dyn RhiDescriptorSetLayout>];
    fn push_constant_ranges(&self) -> &[super::shader::PushConstantRange];
}

pub(crate) trait RhiPipeline: RhiObject {
    fn kind(&self) -> PipelineKind;
    fn layout_id(&self) -> PipelineLayoutId;
}

/// The common layer records strong written-resource references and validates updates.
pub(crate) trait RhiDescriptorSet: RhiObject {
    fn layout(&self) -> &dyn RhiDescriptorSetLayout;
    fn write(&self, writes: &[DescriptorWrite<'_>]) -> RhiResult<()>;
}

pub(crate) trait RhiDescriptorPool: Any + Send + Sync {
    fn allocate(
        &self,
        id: ObjectId,
        layout: &DescriptorSetLayout,
    ) -> RhiResult<Arc<dyn RhiDescriptorSet>>;
    /// Caller proves the pool's submissions completed and invalidates the old generation.
    fn reset(&self) -> RhiResult<()>;
}

pub(crate) trait RhiSurface: Any + Send + Sync {
    fn backend_type(&self) -> BackendType;
}

pub(crate) struct BackendSwapchainImage {
    pub(crate) image: Arc<dyn RhiImage>,
    pub(crate) view: Arc<dyn RhiImageView>,
}

pub(crate) struct BackendAcquiredImage {
    pub(crate) index: u32,
    pub(crate) acquire: AcquireToken,
    pub(crate) present: PresentToken,
    pub(crate) suboptimal: bool,
}

pub(crate) enum BackendAcquireOutcome {
    Acquired(BackendAcquiredImage),
    OutOfDate,
    Timeout,
    SurfaceUnavailable,
}

pub(crate) trait RhiSwapchain: Any + Send {
    fn desc(&self) -> &SwapchainDesc;
    fn images(&self) -> &[BackendSwapchainImage];
    fn acquire(&mut self, timeout: Option<Duration>) -> RhiResult<BackendAcquireOutcome>;
    fn present(&mut self, token: PresentToken) -> RhiResult<PresentOutcome>;
    fn reconfigure(&mut self, width: u32, height: u32) -> RhiResult<()>;
}

pub(crate) trait RhiQueryPool: RhiObject {
    fn desc(&self) -> &QueryPoolDesc;
    /// Nonblocking read; availability is reported per query.
    fn results(&self, queries: Range<u32>) -> RhiResult<Vec<QueryResult>>;
}
