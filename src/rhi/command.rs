#![allow(dead_code)]

use super::descriptor::{DescriptorSet, PipelineLayout};
use super::pipeline::Pipeline;
use super::query::{QueryPool, TimestampStage};
use super::queue::Recorder;
use super::resource::{Buffer, Image, ImageView};
use super::sync::{Access, SyncPoint};
use super::types::*;
use std::ops::Range;

pub struct BufferBinding<'a> {
    pub buffer: &'a Buffer,
    pub offset: u64,
    pub size: u64,
}

pub struct VertexBufferBinding<'a> {
    pub buffer: &'a Buffer,
    pub offset: u64,
}

pub struct IndexBufferBinding<'a> {
    pub buffer: &'a Buffer,
    pub offset: u64,
    pub ty: IndexType,
}

pub struct RenderingAttachment<'a> {
    pub view: &'a ImageView,
    pub load: LoadOp,
    pub store: StoreOp,
    pub clear: Option<ClearValue>,
    pub resolve: Option<ResolveAttachment<'a>>,
}

pub struct ResolveAttachment<'a> {
    pub view: &'a ImageView,
    pub mode: ResolveMode,
}

pub struct RenderingDesc<'a> {
    pub name: &'a str,
    pub area: Scissor,
    pub layer_count: u32,
    pub colors: Vec<RenderingAttachment<'a>>,
    pub depth: Option<RenderingAttachment<'a>>,
    pub stencil: Option<RenderingAttachment<'a>>,
}

pub struct DrawParams<'a> {
    pub pipeline: &'a Pipeline,
    pub vertex_buffers: &'a [VertexBufferBinding<'a>],
    pub index_buffer: Option<IndexBufferBinding<'a>>,
    pub vertex_count: u32,
    pub instance_count: u32,
    pub first_vertex: u32,
    pub first_instance: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct BufferCopy {
    pub src_offset: u64,
    pub dst_offset: u64,
    pub size: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct BufferImageCopy {
    pub buffer_offset: u64,
    pub buffer_row_length: u32,
    pub buffer_image_height: u32,
    pub image_range: ImageSubresourceRange,
    pub image_offset: Offset3d,
    pub extent: Extent3d,
}

#[derive(Clone, Copy, Debug)]
pub struct ImageCopy {
    pub src_range: ImageSubresourceRange,
    pub src_offset: Offset3d,
    pub dst_range: ImageSubresourceRange,
    pub dst_offset: Offset3d,
    pub extent: Extent3d,
}

#[derive(Clone, Copy, Debug)]
pub struct ImageBlit {
    pub src_range: ImageSubresourceRange,
    pub src_offsets: [Offset3d; 2],
    pub dst_range: ImageSubresourceRange,
    pub dst_offsets: [Offset3d; 2],
    pub filter: Filter,
}

pub enum ResourceRegion<'a> {
    Buffer(&'a Buffer, BufferRange),
    Image(&'a Image, ImageSubresourceRange),
}

/// Access intent only; native image layouts remain private to a backend.
pub struct Transition<'a> {
    pub resource: ResourceRegion<'a>,
    pub before: Option<Access>,
    pub after: Access,
}

#[derive(Clone, Copy, Debug)]
pub struct StencilFaceState {
    pub fail: StencilOp,
    pub pass: StencilOp,
    pub depth_fail: StencilOp,
    pub compare: CompareOp,
    pub compare_mask: u32,
    pub write_mask: u32,
    pub reference: u32,
}

#[derive(Clone, Copy, Debug)]
pub enum DynamicState {
    Viewport(Viewport),
    Scissor(Scissor),
    BlendConstants([f32; 4]),
    StencilFront(StencilFaceState),
    StencilBack(StencilFaceState),
    DepthBias {
        constant: f32,
        clamp: f32,
        slope: f32,
    },
    DepthTest(bool),
    DepthWrite(bool),
    DepthCompare(CompareOp),
    CullMode(CullMode),
    FrontFace(FrontFace),
    Topology(PrimitiveTopology),
    PrimitiveRestart(bool),
}

/// Borrowed CPU stream consumed eagerly by build, never retained in a built buffer.
pub enum Command<'a> {
    BeginRendering(RenderingDesc<'a>),
    EndRendering,
    BeginCompute,
    EndCompute,
    BindPipeline(&'a Pipeline),
    BindDescriptorSets {
        layout: &'a PipelineLayout,
        first_set: u32,
        sets: Vec<&'a DescriptorSet>,
        dynamic_offsets: Vec<u32>,
    },
    BindVertexBuffers {
        first_binding: u32,
        buffers: Vec<VertexBufferBinding<'a>>,
    },
    BindIndexBuffer(IndexBufferBinding<'a>),
    PushConstants {
        layout: &'a PipelineLayout,
        stages: ShaderStageFlags,
        offset: u32,
        bytes: Vec<u8>,
    },
    SetDynamicState(DynamicState),
    Draw {
        vertices: Range<u32>,
        instances: Range<u32>,
    },
    DrawIndexed {
        indices: Range<u32>,
        vertex_offset: i32,
        instances: Range<u32>,
    },
    DrawIndirect {
        buffer: &'a Buffer,
        offset: u64,
        draw_count: u32,
        stride: u32,
    },
    DrawIndexedIndirect {
        buffer: &'a Buffer,
        offset: u64,
        draw_count: u32,
        stride: u32,
    },
    Dispatch([u32; 3]),
    DispatchIndirect {
        buffer: &'a Buffer,
        offset: u64,
    },
    CopyBuffer {
        src: &'a Buffer,
        dst: &'a Buffer,
        regions: Vec<BufferCopy>,
    },
    CopyBufferToImage {
        src: &'a Buffer,
        dst: &'a Image,
        regions: Vec<BufferImageCopy>,
    },
    CopyImageToBuffer {
        src: &'a Image,
        dst: &'a Buffer,
        regions: Vec<BufferImageCopy>,
    },
    CopyImage {
        src: &'a Image,
        dst: &'a Image,
        regions: Vec<ImageCopy>,
    },
    BlitImage {
        src: &'a Image,
        dst: &'a Image,
        regions: Vec<ImageBlit>,
    },
    UpdateBuffer {
        dst: &'a Buffer,
        offset: u64,
        bytes: Vec<u8>,
    },
    FillBuffer {
        dst: &'a Buffer,
        range: BufferRange,
        value: u32,
    },
    UseResource {
        resource: ResourceRegion<'a>,
        access: Access,
    },
    ReleaseBuffers {
        destination: &'a super::queue::Queue,
        buffers: Vec<(&'a Buffer, BufferRange)>,
    },
    ReleaseImages {
        destination: &'a super::queue::Queue,
        images: Vec<(&'a Image, ImageSubresourceRange)>,
    },
    WaitOn(SyncPoint),
    ResetQueries {
        pool: &'a QueryPool,
        queries: Range<u32>,
    },
    WriteTimestamp {
        pool: &'a QueryPool,
        query: u32,
        stage: TimestampStage,
    },
    BeginDebugLabel {
        name: String,
        color: [f32; 4],
    },
    EndDebugLabel,
    InsertDebugLabel {
        name: String,
        color: [f32; 4],
    },
}

/// Encoder operation methods will be added with the public facade implementation.
pub struct RenderPassEncoder<'pass, 'resource> {
    pub(crate) recorder: &'pass mut Recorder<'resource>,
}

pub struct ComputePassEncoder<'pass, 'resource> {
    pub(crate) recorder: &'pass mut Recorder<'resource>,
}
