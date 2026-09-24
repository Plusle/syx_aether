#![allow(dead_code)]

use crate::rhi::{
    descriptor::DescriptorSet,
    error::RhiResult,
    pipeline::Pipeline,
    queue::QueueKind,
    resource::{Buffer, Image, ImageView, PlannedOp, Transition},
    shader::ShaderStageFlags,
    types::RhiType,
};

pub use crate::rhi::resource::ImageAspect;

#[derive(Debug, Clone, Copy)]
pub struct ColorAttachment<'a> {
    pub view: &'a ImageView,
    pub resolve: Option<&'a ImageView>,
    pub load_op: LoadOp,
    pub store_op: StoreOp,
}

#[derive(Debug, Clone, Copy)]
pub struct DepthStencilAttachment<'a> {
    pub view: &'a ImageView,
    pub resolve: Option<&'a ImageView>,
    pub depth_load_op: LoadOp,
    pub depth_store_op: StoreOp,
    pub stencil_load_op: LoadOp,
    pub stencil_store_op: StoreOp,
}

#[derive(Debug, Clone, Copy)]
pub struct RenderingDesc<'a> {
    pub color_attachments: &'a [ColorAttachment<'a>],
    pub depth_stencil: Option<&'a DepthStencilAttachment<'a>>,
    pub extent: [u32; 2],
    pub layer_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LoadOp {
    Load,
    Clear(ClearValue),
    DontCare,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StoreOp {
    Store,
    DontCare,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ClearValue {
    Color([f32; 4]),
    DepthStencil { depth: f32, stencil: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BufferImageCopy {
    pub buffer_offset: u64,
    pub image_subresource: ImageSubresourceLayers,
    pub image_offset: [i32; 3],
    pub image_extent: [u32; 3],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImageSubresourceLayers {
    pub aspect: ImageAspect,
    pub mip: u32,
    pub base_layer: u32,
    pub layer_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImageCopyRegion {
    pub subresource: ImageSubresourceLayers,
    pub offset: [i32; 3],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub min_depth: f32,
    pub max_depth: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexType {
    U16,
    U32,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum Command<'a> {
    BeginRendering(RenderingDesc<'a>),
    EndRendering,
    Draw {
        vertex_count: u32,
        instance_count: u32,
        first_vertex: u32,
        first_instance: u32,
    },
    DrawIndexed {
        index_count: u32,
        instance_count: u32,
        first_index: u32,
        vertex_offset: i32,
        first_instance: u32,
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
        src_offset: u64,
        dst: &'a Buffer,
        dst_offset: u64,
        size: u64,
    },
    CopyBufferToImage {
        src: &'a Buffer,
        dst: &'a Image,
        region: BufferImageCopy,
    },
    CopyImageToBuffer {
        src: &'a Image,
        dst: &'a Buffer,
        region: BufferImageCopy,
    },
    CopyImage {
        src: &'a Image,
        src_region: ImageCopyRegion,
        dst: &'a Image,
        dst_region: ImageCopyRegion,
        extent: [u32; 3],
    },
    ReleaseBuffers {
        buffers: &'a [&'a Buffer],
        target: QueueKind,
    },
    ReleaseImages {
        images: &'a [&'a Image],
        target: QueueKind,
    },
    BindPipeline(&'a Pipeline),
    BindVertexBuffers {
        first: u32,
        buffers: &'a [(&'a Buffer, u64)],
    },
    BindIndexBuffer {
        buffer: &'a Buffer,
        offset: u64,
        index_type: IndexType,
    },
    BindDescriptorSets {
        pipeline: &'a Pipeline,
        first_set: u32,
        sets: &'a [&'a DescriptorSet],
    },
    PushConstants {
        pipeline: &'a Pipeline,
        stages: ShaderStageFlags,
        offset: u32,
        data: &'a [u8],
    },
    SetViewport(Viewport),
    SetScissor(Rect),
    SetBlendConstants([f32; 4]),
    SetStencilReference(u32),
    BeginDebugLabel(&'a str),
    EndDebugLabel,
    InsertDebugLabel(&'a str),
}

pub(crate) trait RhiCommandBuffer: RhiType {
    fn reset(&mut self) -> RhiResult<()>;

    fn begin(&mut self) -> RhiResult<()>;

    fn finish(&mut self) -> RhiResult<()>;

    fn barrier(&mut self, descs: &[Transition<'_>]);

    fn begin_rendering(&mut self, desc: &RenderingDesc<'_>);

    fn end_rendering(&mut self);

    fn draw(
        &mut self,
        vertex_count: u32,
        instance_count: u32,
        first_vertex: u32,
        first_instance: u32,
    );

    fn draw_indexed(
        &mut self,
        index_count: u32,
        instance_count: u32,
        first_index: u32,
        vertex_offset: i32,
        first_instance: u32,
    );

    fn draw_indirect(&mut self, buffer: &Buffer, offset: u64, draw_count: u32, stride: u32);

    fn draw_indexed_indirect(&mut self, buffer: &Buffer, offset: u64, draw_count: u32, stride: u32);

    fn dispatch(&mut self, group_count: [u32; 3]);

    fn dispatch_indirect(&mut self, buffer: &Buffer, offset: u64);

    fn copy_buffer(
        &mut self,
        src: &Buffer,
        src_offset: u64,
        dst: &Buffer,
        dst_offset: u64,
        size: u64,
    );

    fn copy_buffer_to_image(&mut self, src: &Buffer, dst: &Image, region: &BufferImageCopy);

    fn copy_image_to_buffer(&mut self, src: &Image, dst: &Buffer, region: &BufferImageCopy);

    fn copy_image(
        &mut self,
        src: &Image,
        src_region: &ImageCopyRegion,
        dst: &Image,
        dst_region: &ImageCopyRegion,
        extent: [u32; 3],
    );

    fn bind_pipeline(&mut self, pipeline: &Pipeline);

    fn bind_vertex_buffers(&mut self, first: u32, buffers: &[(&Buffer, u64)]);

    fn bind_index_buffer(&mut self, buffer: &Buffer, offset: u64, index_type: IndexType);

    fn bind_descriptor_sets(
        &mut self,
        pipeline: &Pipeline,
        first_set: u32,
        sets: &[&DescriptorSet],
    );

    fn push_constants(
        &mut self,
        pipeline: &Pipeline,
        stages: ShaderStageFlags,
        offset: u32,
        data: &[u8],
    );

    fn set_viewport(&mut self, viewport: &Viewport);

    fn set_scissor(&mut self, scissor: &Rect);

    fn set_blend_constants(&mut self, constants: [f32; 4]);

    fn set_stencil_reference(&mut self, reference: u32);

    fn begin_debug_label(&mut self, name: &str);

    fn end_debug_label(&mut self);

    fn insert_debug_label(&mut self, name: &str);
}

pub(crate) fn record_planned(
    buffer: &mut dyn RhiCommandBuffer,
    stream: &[PlannedOp<'_>],
) -> RhiResult<()> {
    buffer.begin()?;
    for operation in stream {
        match operation {
            PlannedOp::Barriers(transitions) => buffer.barrier(transitions),
            PlannedOp::Command(command) => record_command(buffer, *command),
        }
    }
    buffer.finish()
}

fn record_command(buffer: &mut dyn RhiCommandBuffer, command: Command<'_>) {
    match command {
        Command::BeginRendering(desc) => buffer.begin_rendering(&desc),
        Command::EndRendering => buffer.end_rendering(),
        Command::Draw {
            vertex_count,
            instance_count,
            first_vertex,
            first_instance,
        } => buffer.draw(vertex_count, instance_count, first_vertex, first_instance),
        Command::DrawIndexed {
            index_count,
            instance_count,
            first_index,
            vertex_offset,
            first_instance,
        } => buffer.draw_indexed(
            index_count,
            instance_count,
            first_index,
            vertex_offset,
            first_instance,
        ),
        Command::DrawIndirect {
            buffer: source,
            offset,
            draw_count,
            stride,
        } => buffer.draw_indirect(source, offset, draw_count, stride),
        Command::DrawIndexedIndirect {
            buffer: source,
            offset,
            draw_count,
            stride,
        } => buffer.draw_indexed_indirect(source, offset, draw_count, stride),
        Command::Dispatch(count) => buffer.dispatch(count),
        Command::DispatchIndirect {
            buffer: source,
            offset,
        } => buffer.dispatch_indirect(source, offset),
        Command::CopyBuffer {
            src,
            src_offset,
            dst,
            dst_offset,
            size,
        } => buffer.copy_buffer(src, src_offset, dst, dst_offset, size),
        Command::CopyBufferToImage { src, dst, region } => {
            buffer.copy_buffer_to_image(src, dst, &region)
        }
        Command::CopyImageToBuffer { src, dst, region } => {
            buffer.copy_image_to_buffer(src, dst, &region)
        }
        Command::CopyImage {
            src,
            src_region,
            dst,
            dst_region,
            extent,
        } => buffer.copy_image(src, &src_region, dst, &dst_region, extent),
        Command::ReleaseBuffers { .. } | Command::ReleaseImages { .. } => {
            unreachable!("release commands are consumed by the resource planner")
        }
        Command::BindPipeline(pipeline) => buffer.bind_pipeline(pipeline),
        Command::BindVertexBuffers { first, buffers } => buffer.bind_vertex_buffers(first, buffers),
        Command::BindIndexBuffer {
            buffer: source,
            offset,
            index_type,
        } => buffer.bind_index_buffer(source, offset, index_type),
        Command::BindDescriptorSets {
            pipeline,
            first_set,
            sets,
        } => buffer.bind_descriptor_sets(pipeline, first_set, sets),
        Command::PushConstants {
            pipeline,
            stages,
            offset,
            data,
        } => buffer.push_constants(pipeline, stages, offset, data),
        Command::SetViewport(viewport) => buffer.set_viewport(&viewport),
        Command::SetScissor(scissor) => buffer.set_scissor(&scissor),
        Command::SetBlendConstants(constants) => buffer.set_blend_constants(constants),
        Command::SetStencilReference(reference) => buffer.set_stencil_reference(reference),
        Command::BeginDebugLabel(name) => buffer.begin_debug_label(name),
        Command::EndDebugLabel => buffer.end_debug_label(),
        Command::InsertDebugLabel(name) => buffer.insert_debug_label(name),
    }
}
