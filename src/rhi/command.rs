#![allow(dead_code)]

use crate::rhi::{
    descriptor::RhiDescriptorSet,
    pipeline::RhiPipeline,
    resource::{QueueKind, RhiBuffer, RhiImage, RhiImageView, Transition},
    shader::ShaderStageFlags,
    types::RhiType,
};

pub use crate::rhi::resource::ImageAspect;

/// One color attachment of a dynamic rendering pass.
#[derive(Debug, Clone, Copy)]
pub struct ColorAttachment<'a> {
    pub view: &'a dyn RhiImageView,
    pub resolve: Option<&'a dyn RhiImageView>,
    pub load_op: LoadOp,
    pub store_op: StoreOp,
}

/// Depth/stencil attachment of a dynamic rendering pass.
#[derive(Debug, Clone, Copy)]
pub struct DepthStencilAttachment<'a> {
    pub view: &'a dyn RhiImageView,
    pub resolve: Option<&'a dyn RhiImageView>,
    pub depth_load_op: LoadOp,
    pub depth_store_op: StoreOp,
    pub stencil_load_op: LoadOp,
    pub stencil_store_op: StoreOp,
}

/// Description of one dynamic rendering pass.
#[derive(Debug, Clone, Copy)]
pub struct RenderingDesc<'a> {
    pub color_attachments: &'a [ColorAttachment<'a>],
    pub depth_stencil: Option<&'a DepthStencilAttachment<'a>>,
    pub extent: [u32; 2],
    pub layer_count: u32,
}

/// What to do with an attachment's previous contents.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LoadOp {
    Load,
    Clear(ClearValue),
    DontCare,
}

/// What to do with an attachment's produced contents.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StoreOp {
    Store,
    DontCare,
}

/// Clear value for an attachment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ClearValue {
    Color([f32; 4]),
    DepthStencil { depth: f32, stencil: u32 },
}

/// One buffer-to-image or image-to-buffer copy region.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BufferImageCopy {
    pub buffer_offset: u64,
    pub image_subresource: ImageSubresourceLayers,
    pub image_offset: [i32; 3],
    pub image_extent: [u32; 3],
}

/// Image subresource selection for copy operations.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImageSubresourceLayers {
    pub aspect: ImageAspect,
    pub mip: u32,
    pub base_layer: u32,
    pub layer_count: u32,
}

/// Source or destination corner of one image-to-image copy region.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImageCopyRegion {
    pub subresource: ImageSubresourceLayers,
    pub offset: [i32; 3],
}

/// Viewport rectangle and depth range.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub min_depth: f32,
    pub max_depth: f32,
}

/// Scissor or other integer rectangle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Index element type of an index buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexType {
    U16,
    U32,
}

/// A backend-neutral command description.
#[derive(Debug, Clone, Copy)]
pub enum Command<'a> {
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
        buffer: &'a dyn RhiBuffer,
        offset: u64,
        draw_count: u32,
        stride: u32,
    },
    DrawIndexedIndirect {
        buffer: &'a dyn RhiBuffer,
        offset: u64,
        draw_count: u32,
        stride: u32,
    },
    Dispatch([u32; 3]),
    DispatchIndirect {
        buffer: &'a dyn RhiBuffer,
        offset: u64,
    },
    CopyBuffer {
        src: &'a dyn RhiBuffer,
        src_offset: u64,
        dst: &'a dyn RhiBuffer,
        dst_offset: u64,
        size: u64,
    },
    CopyBufferToImage {
        src: &'a dyn RhiBuffer,
        dst: &'a dyn RhiImage,
        region: BufferImageCopy,
    },
    CopyImageToBuffer {
        src: &'a dyn RhiImage,
        dst: &'a dyn RhiBuffer,
        region: BufferImageCopy,
    },
    CopyImage {
        src: &'a dyn RhiImage,
        src_region: ImageCopyRegion,
        dst: &'a dyn RhiImage,
        dst_region: ImageCopyRegion,
        extent: [u32; 3],
    },
    ReleaseBuffers {
        buffers: &'a [&'a dyn RhiBuffer],
        target: QueueKind,
    },
    ReleaseImages {
        images: &'a [&'a dyn RhiImage],
        target: QueueKind,
    },
    BindPipeline(&'a dyn RhiPipeline),
    BindVertexBuffers {
        first: u32,
        buffers: &'a [(&'a dyn RhiBuffer, u64)],
    },
    BindIndexBuffer {
        buffer: &'a dyn RhiBuffer,
        offset: u64,
        index_type: IndexType,
    },
    BindDescriptorSets {
        pipeline: &'a dyn RhiPipeline,
        first_set: u32,
        sets: &'a [&'a dyn RhiDescriptorSet],
    },
    PushConstants {
        pipeline: &'a dyn RhiPipeline,
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

/// Backend implementation of a command buffer.
pub(crate) trait RhiCommandBuffer: RhiType {
    /// Begin recording.
    fn begin(&mut self);

    /// Finish recording; the sealed buffer goes to the submit path.
    fn finish(&mut self);

    /// Record one native barrier from tracker-derived descs.
    /// Crate-internal contract: only the recorder calls this, with the
    /// transition list produced by `ResourceTracker::plan`.
    fn barrier(&mut self, descs: &[Transition<'_>]);

    /// Begin a dynamic rendering pass.
    fn begin_rendering(&mut self, desc: &RenderingDesc<'_>);

    /// End the current dynamic rendering pass.
    fn end_rendering(&mut self);

    /// Non-indexed draw.
    fn draw(
        &mut self,
        vertex_count: u32,
        instance_count: u32,
        first_vertex: u32,
        first_instance: u32,
    );

    /// Indexed draw.
    fn draw_indexed(
        &mut self,
        index_count: u32,
        instance_count: u32,
        first_index: u32,
        vertex_offset: i32,
        first_instance: u32,
    );

    /// Indirect non-indexed draw.
    fn draw_indirect(&mut self, buffer: &dyn RhiBuffer, offset: u64, draw_count: u32, stride: u32);

    /// Indirect indexed draw.
    fn draw_indexed_indirect(
        &mut self,
        buffer: &dyn RhiBuffer,
        offset: u64,
        draw_count: u32,
        stride: u32,
    );

    /// Compute dispatch.
    fn dispatch(&mut self, group_count: [u32; 3]);

    /// Indirect compute dispatch.
    fn dispatch_indirect(&mut self, buffer: &dyn RhiBuffer, offset: u64);

    /// Copy a range between two buffers.
    fn copy_buffer(
        &mut self,
        src: &dyn RhiBuffer,
        src_offset: u64,
        dst: &dyn RhiBuffer,
        dst_offset: u64,
        size: u64,
    );

    /// Copy one region of a buffer into an image.
    fn copy_buffer_to_image(
        &mut self,
        src: &dyn RhiBuffer,
        dst: &dyn RhiImage,
        region: &BufferImageCopy,
    );

    /// Copy one region of an image into a buffer.
    fn copy_image_to_buffer(
        &mut self,
        src: &dyn RhiImage,
        dst: &dyn RhiBuffer,
        region: &BufferImageCopy,
    );

    /// Copy a box between two images.
    fn copy_image(
        &mut self,
        src: &dyn RhiImage,
        src_region: &ImageCopyRegion,
        dst: &dyn RhiImage,
        dst_region: &ImageCopyRegion,
        extent: [u32; 3],
    );

    /// Bind a graphics or compute pipeline.
    fn bind_pipeline(&mut self, pipeline: &dyn RhiPipeline);

    /// Bind vertex buffers starting at slot `first`.
    fn bind_vertex_buffers(&mut self, first: u32, buffers: &[(&dyn RhiBuffer, u64)]);

    /// Bind the index buffer.
    fn bind_index_buffer(&mut self, buffer: &dyn RhiBuffer, offset: u64, index_type: IndexType);

    /// Bind descriptor sets for the pipeline's layout.
    fn bind_descriptor_sets(
        &mut self,
        pipeline: &dyn RhiPipeline,
        first_set: u32,
        sets: &[&dyn RhiDescriptorSet],
    );

    /// Push shader constants for the given stages.
    fn push_constants(
        &mut self,
        pipeline: &dyn RhiPipeline,
        stages: ShaderStageFlags,
        offset: u32,
        data: &[u8],
    );

    /// Set the viewport.
    fn set_viewport(&mut self, viewport: &Viewport);

    /// Set the scissor rectangle.
    fn set_scissor(&mut self, scissor: &Rect);

    /// Set blend constants.
    fn set_blend_constants(&mut self, constants: [f32; 4]);

    /// Set the stencil reference value.
    fn set_stencil_reference(&mut self, reference: u32);

    /// Begin a nested debug label region.
    fn begin_debug_label(&mut self, name: &str);

    /// End the current debug label region.
    fn end_debug_label(&mut self);

    /// Insert a single debug label.
    fn insert_debug_label(&mut self, name: &str);
}
