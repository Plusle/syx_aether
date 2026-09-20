#![allow(dead_code)]

use super::{
    descriptor::VkDescriptorSet,
    pipeline::VkPipeline,
    queue::VkQueueRegistry,
    resource::{VkBuffer, VkImage, VkImageView},
};
use crate::rhi::{
    command::{
        BufferImageCopy, ClearValue, ImageAspect, ImageCopyRegion, ImageSubresourceLayers,
        IndexType, LoadOp, Rect, RenderingDesc, RhiCommandBuffer, StoreOp, Viewport,
    },
    descriptor::RhiDescriptorSet,
    error::RhiResult,
    pipeline::RhiPipeline,
    resource::{
        Access, ResourceRange, ResourceRef, ResourceState, RhiBuffer, RhiImage, RhiImageView,
        Transition, TransitionKind,
    },
    shader::ShaderStageFlags,
};
use ash::vk;
use std::{
    fmt::{Debug, Formatter},
    sync::Arc,
};

/// A non-owning Vulkan command-buffer recorder.
///
/// The command pool owns `handle`; the cloned Ash device is only a dispatch
/// table for recording commands.
pub(crate) struct VkCommandBuffer {
    handle: vk::CommandBuffer,
    device: ash::Device,
    queues: Arc<VkQueueRegistry>,
}

impl VkCommandBuffer {
    pub(crate) fn new(
        device: ash::Device,
        handle: vk::CommandBuffer,
        queues: Arc<VkQueueRegistry>,
    ) -> Self {
        Self {
            handle,
            device,
            queues,
        }
    }

    pub(crate) fn handle(&self) -> vk::CommandBuffer {
        self.handle
    }
}

impl Debug for VkCommandBuffer {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VkCommandBuffer")
            .field("handle", &self.handle)
            .finish()
    }
}

impl RhiCommandBuffer for VkCommandBuffer {
    fn reset(&mut self) -> RhiResult<()> {
        unsafe {
            self.device
                .reset_command_buffer(self.handle, vk::CommandBufferResetFlags::empty())
        }?;
        Ok(())
    }

    fn begin(&mut self) -> RhiResult<()> {
        let info = vk::CommandBufferBeginInfo::default();
        unsafe { self.device.begin_command_buffer(self.handle, &info) }?;
        Ok(())
    }

    fn finish(&mut self) -> RhiResult<()> {
        unsafe { self.device.end_command_buffer(self.handle) }?;
        Ok(())
    }

    fn barrier(&mut self, descs: &[Transition<'_>]) {
        let mut buffers = Vec::new();
        let mut images = Vec::new();
        for transition in descs {
            let (mut src_stage, mut src_access) = transition
                .before
                .map(map_state)
                .unwrap_or((vk::PipelineStageFlags2::NONE, vk::AccessFlags2::NONE));
            let (mut dst_stage, mut dst_access) = map_state(transition.after);
            match transition.kind {
                TransitionKind::Memory => {}
                TransitionKind::Release => {
                    dst_stage = vk::PipelineStageFlags2::NONE;
                    dst_access = vk::AccessFlags2::NONE;
                }
                TransitionKind::Acquire => {
                    src_stage = vk::PipelineStageFlags2::NONE;
                    src_access = vk::AccessFlags2::NONE;
                }
            }
            let (src_queue_family_index, dst_queue_family_index) = match transition.kind {
                TransitionKind::Memory => (vk::QUEUE_FAMILY_IGNORED, vk::QUEUE_FAMILY_IGNORED),
                TransitionKind::Release | TransitionKind::Acquire => {
                    let source = transition
                        .before
                        .map(|state| self.queues.family(state.queue))
                        .unwrap_or(vk::QUEUE_FAMILY_IGNORED);
                    let target = self.queues.family(transition.after.queue);
                    if source == target {
                        (vk::QUEUE_FAMILY_IGNORED, vk::QUEUE_FAMILY_IGNORED)
                    } else {
                        (source, target)
                    }
                }
            };
            match (transition.resource, transition.range) {
                (ResourceRef::Buffer(buffer), ResourceRange::Buffer { offset, size }) => buffers
                    .push(
                        vk::BufferMemoryBarrier2::default()
                            .src_stage_mask(src_stage)
                            .src_access_mask(src_access)
                            .dst_stage_mask(dst_stage)
                            .dst_access_mask(dst_access)
                            .src_queue_family_index(src_queue_family_index)
                            .dst_queue_family_index(dst_queue_family_index)
                            .buffer(buffer_handle(buffer))
                            .offset(offset)
                            .size(size),
                    ),
                (resource, ResourceRange::Image(range)) => images.push(
                    vk::ImageMemoryBarrier2::default()
                        .src_stage_mask(src_stage)
                        .src_access_mask(src_access)
                        .dst_stage_mask(dst_stage)
                        .dst_access_mask(dst_access)
                        .old_layout(
                            transition
                                .before
                                .map(map_image_layout)
                                .unwrap_or(vk::ImageLayout::UNDEFINED),
                        )
                        .new_layout(map_image_layout(transition.after))
                        .src_queue_family_index(src_queue_family_index)
                        .dst_queue_family_index(dst_queue_family_index)
                        .image(resource_image_handle(resource))
                        .subresource_range(vk::ImageSubresourceRange {
                            aspect_mask: map_aspect(range.aspect),
                            base_mip_level: range.base_mip,
                            level_count: range.mip_count,
                            base_array_layer: range.base_layer,
                            layer_count: range.layer_count,
                        }),
                ),
                _ => unreachable!("resource reference and range kinds must match"),
            }
        }
        let dependency = vk::DependencyInfo::default()
            .buffer_memory_barriers(&buffers)
            .image_memory_barriers(&images);
        unsafe { self.device.cmd_pipeline_barrier2(self.handle, &dependency) };
    }

    fn begin_rendering(&mut self, desc: &RenderingDesc<'_>) {
        let colors: Vec<_> = desc
            .color_attachments
            .iter()
            .map(|attachment| {
                let mut info = vk::RenderingAttachmentInfo::default()
                    .image_view(image_view(attachment.view).handle())
                    .image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
                    .load_op(map_load_op(attachment.load_op))
                    .store_op(map_store_op(attachment.store_op))
                    .clear_value(map_clear_value(attachment.load_op));
                if let Some(resolve) = attachment.resolve {
                    info = info
                        .resolve_mode(vk::ResolveModeFlags::AVERAGE)
                        .resolve_image_view(image_view(resolve).handle())
                        .resolve_image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
                }
                info
            })
            .collect();

        let depth = desc.depth_stencil.map(|attachment| {
            depth_stencil_attachment(
                attachment.view,
                attachment.resolve,
                attachment.depth_load_op,
                attachment.depth_store_op,
            )
        });
        let stencil = desc.depth_stencil.map(|attachment| {
            depth_stencil_attachment(
                attachment.view,
                attachment.resolve,
                attachment.stencil_load_op,
                attachment.stencil_store_op,
            )
        });

        let mut info = vk::RenderingInfo::default()
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: vk::Extent2D {
                    width: desc.extent[0],
                    height: desc.extent[1],
                },
            })
            .layer_count(desc.layer_count)
            .color_attachments(&colors);
        if let Some(depth) = &depth {
            info = info.depth_attachment(depth);
        }
        if let Some(stencil) = &stencil {
            info = info.stencil_attachment(stencil);
        }
        unsafe { self.device.cmd_begin_rendering(self.handle, &info) };
    }

    fn end_rendering(&mut self) {
        unsafe { self.device.cmd_end_rendering(self.handle) };
    }

    fn draw(
        &mut self,
        vertex_count: u32,
        instance_count: u32,
        first_vertex: u32,
        first_instance: u32,
    ) {
        unsafe {
            self.device.cmd_draw(
                self.handle,
                vertex_count,
                instance_count,
                first_vertex,
                first_instance,
            )
        };
    }

    fn draw_indexed(
        &mut self,
        index_count: u32,
        instance_count: u32,
        first_index: u32,
        vertex_offset: i32,
        first_instance: u32,
    ) {
        unsafe {
            self.device.cmd_draw_indexed(
                self.handle,
                index_count,
                instance_count,
                first_index,
                vertex_offset,
                first_instance,
            )
        };
    }

    fn draw_indirect(&mut self, buffer: &dyn RhiBuffer, offset: u64, draw_count: u32, stride: u32) {
        unsafe {
            self.device.cmd_draw_indirect(
                self.handle,
                buffer_handle(buffer),
                offset,
                draw_count,
                stride,
            )
        };
    }

    fn draw_indexed_indirect(
        &mut self,
        buffer: &dyn RhiBuffer,
        offset: u64,
        draw_count: u32,
        stride: u32,
    ) {
        unsafe {
            self.device.cmd_draw_indexed_indirect(
                self.handle,
                buffer_handle(buffer),
                offset,
                draw_count,
                stride,
            )
        };
    }

    fn dispatch(&mut self, count: [u32; 3]) {
        unsafe {
            self.device
                .cmd_dispatch(self.handle, count[0], count[1], count[2])
        };
    }

    fn dispatch_indirect(&mut self, buffer: &dyn RhiBuffer, offset: u64) {
        unsafe {
            self.device
                .cmd_dispatch_indirect(self.handle, buffer_handle(buffer), offset)
        };
    }

    fn copy_buffer(
        &mut self,
        src: &dyn RhiBuffer,
        src_offset: u64,
        dst: &dyn RhiBuffer,
        dst_offset: u64,
        size: u64,
    ) {
        let region = vk::BufferCopy::default()
            .src_offset(src_offset)
            .dst_offset(dst_offset)
            .size(size);
        unsafe {
            self.device.cmd_copy_buffer(
                self.handle,
                buffer_handle(src),
                buffer_handle(dst),
                &[region],
            )
        };
    }

    fn copy_buffer_to_image(
        &mut self,
        src: &dyn RhiBuffer,
        dst: &dyn RhiImage,
        region: &BufferImageCopy,
    ) {
        unsafe {
            self.device.cmd_copy_buffer_to_image(
                self.handle,
                buffer_handle(src),
                image(dst).handle(),
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[map_buffer_image_copy(region)],
            )
        };
    }

    fn copy_image_to_buffer(
        &mut self,
        src: &dyn RhiImage,
        dst: &dyn RhiBuffer,
        region: &BufferImageCopy,
    ) {
        unsafe {
            self.device.cmd_copy_image_to_buffer(
                self.handle,
                image(src).handle(),
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                buffer_handle(dst),
                &[map_buffer_image_copy(region)],
            )
        };
    }

    fn copy_image(
        &mut self,
        src: &dyn RhiImage,
        src_region: &ImageCopyRegion,
        dst: &dyn RhiImage,
        dst_region: &ImageCopyRegion,
        extent: [u32; 3],
    ) {
        let region = vk::ImageCopy::default()
            .src_subresource(map_subresource(src_region.subresource))
            .src_offset(map_offset(src_region.offset))
            .dst_subresource(map_subresource(dst_region.subresource))
            .dst_offset(map_offset(dst_region.offset))
            .extent(vk::Extent3D {
                width: extent[0],
                height: extent[1],
                depth: extent[2],
            });
        unsafe {
            self.device.cmd_copy_image(
                self.handle,
                image(src).handle(),
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                image(dst).handle(),
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[region],
            )
        };
    }

    fn bind_pipeline(&mut self, pipeline: &dyn RhiPipeline) {
        let pipeline = vk_pipeline(pipeline);
        unsafe {
            self.device
                .cmd_bind_pipeline(self.handle, pipeline.bind_point, pipeline.handle)
        };
    }

    fn bind_vertex_buffers(&mut self, first: u32, buffers: &[(&dyn RhiBuffer, u64)]) {
        let handles: Vec<_> = buffers
            .iter()
            .map(|(buffer, _)| buffer_handle(*buffer))
            .collect();
        let offsets: Vec<_> = buffers.iter().map(|(_, offset)| *offset).collect();
        unsafe {
            self.device
                .cmd_bind_vertex_buffers(self.handle, first, &handles, &offsets)
        };
    }

    fn bind_index_buffer(&mut self, buffer: &dyn RhiBuffer, offset: u64, ty: IndexType) {
        let ty = match ty {
            IndexType::U16 => vk::IndexType::UINT16,
            IndexType::U32 => vk::IndexType::UINT32,
        };
        unsafe {
            self.device
                .cmd_bind_index_buffer(self.handle, buffer_handle(buffer), offset, ty)
        };
    }

    fn bind_descriptor_sets(
        &mut self,
        pipeline: &dyn RhiPipeline,
        first_set: u32,
        sets: &[&dyn RhiDescriptorSet],
    ) {
        let pipeline = vk_pipeline(pipeline);
        let sets: Vec<_> = sets.iter().map(|set| descriptor_set(*set).handle).collect();
        unsafe {
            self.device.cmd_bind_descriptor_sets(
                self.handle,
                pipeline.bind_point,
                pipeline.layout,
                first_set,
                &sets,
                &[],
            )
        };
    }

    fn push_constants(
        &mut self,
        pipeline: &dyn RhiPipeline,
        stages: ShaderStageFlags,
        offset: u32,
        data: &[u8],
    ) {
        unsafe {
            self.device.cmd_push_constants(
                self.handle,
                vk_pipeline(pipeline).layout,
                map_shader_stages(stages),
                offset,
                data,
            )
        };
    }

    fn set_viewport(&mut self, viewport: &Viewport) {
        let viewport = vk::Viewport {
            x: viewport.x,
            y: viewport.y,
            width: viewport.width,
            height: viewport.height,
            min_depth: viewport.min_depth,
            max_depth: viewport.max_depth,
        };
        unsafe { self.device.cmd_set_viewport(self.handle, 0, &[viewport]) };
    }

    fn set_scissor(&mut self, scissor: &Rect) {
        let scissor = vk::Rect2D {
            offset: vk::Offset2D {
                x: scissor.x,
                y: scissor.y,
            },
            extent: vk::Extent2D {
                width: scissor.width,
                height: scissor.height,
            },
        };
        unsafe { self.device.cmd_set_scissor(self.handle, 0, &[scissor]) };
    }

    fn set_blend_constants(&mut self, constants: [f32; 4]) {
        unsafe { self.device.cmd_set_blend_constants(self.handle, &constants) };
    }

    fn set_stencil_reference(&mut self, reference: u32) {
        unsafe {
            self.device.cmd_set_stencil_reference(
                self.handle,
                vk::StencilFaceFlags::FRONT_AND_BACK,
                reference,
            )
        };
    }

    // Debug labels require VK_EXT_debug_utils device dispatch, which is not
    // retained by the current device context.
    fn begin_debug_label(&mut self, _name: &str) {}
    fn end_debug_label(&mut self) {}
    fn insert_debug_label(&mut self, _name: &str) {}
}

fn buffer_handle(buffer: &dyn RhiBuffer) -> vk::Buffer {
    buffer
        .as_any()
        .downcast_ref::<VkBuffer>()
        .expect("Vulkan command buffer received a non-Vulkan buffer")
        .handle()
}

fn image(image: &dyn RhiImage) -> &VkImage {
    image
        .as_any()
        .downcast_ref::<VkImage>()
        .expect("Vulkan command buffer received a non-Vulkan image")
}

fn image_view(view: &dyn RhiImageView) -> &VkImageView {
    view.as_any()
        .downcast_ref::<VkImageView>()
        .expect("Vulkan command buffer received a non-Vulkan image view")
}

fn resource_image_handle(resource: ResourceRef<'_>) -> vk::Image {
    match resource {
        ResourceRef::Image(image) => self::image(image).handle(),
        ResourceRef::ImageView(view) => image_view(view).image_handle,
        ResourceRef::Buffer(_) => {
            unreachable!("buffer cannot back an image transition")
        }
    }
}

fn map_state(state: ResourceState) -> (vk::PipelineStageFlags2, vk::AccessFlags2) {
    match state.access {
        Access::VertexRead => (
            vk::PipelineStageFlags2::VERTEX_INPUT,
            vk::AccessFlags2::VERTEX_ATTRIBUTE_READ,
        ),
        Access::IndexRead => (
            vk::PipelineStageFlags2::INDEX_INPUT,
            vk::AccessFlags2::INDEX_READ,
        ),
        Access::IndirectRead => (
            vk::PipelineStageFlags2::DRAW_INDIRECT,
            vk::AccessFlags2::INDIRECT_COMMAND_READ,
        ),
        Access::TransferRead => (
            vk::PipelineStageFlags2::TRANSFER,
            vk::AccessFlags2::TRANSFER_READ,
        ),
        Access::TransferWrite => (
            vk::PipelineStageFlags2::TRANSFER,
            vk::AccessFlags2::TRANSFER_WRITE,
        ),
        Access::ShaderRead => (
            vk::PipelineStageFlags2::ALL_GRAPHICS | vk::PipelineStageFlags2::COMPUTE_SHADER,
            vk::AccessFlags2::SHADER_READ,
        ),
        Access::ShaderWrite => (
            vk::PipelineStageFlags2::ALL_GRAPHICS | vk::PipelineStageFlags2::COMPUTE_SHADER,
            vk::AccessFlags2::SHADER_WRITE,
        ),
        Access::ColorAttachmentReadWrite => (
            vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
            vk::AccessFlags2::COLOR_ATTACHMENT_READ | vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
        ),
        Access::DepthStencilReadWrite => (
            vk::PipelineStageFlags2::EARLY_FRAGMENT_TESTS
                | vk::PipelineStageFlags2::LATE_FRAGMENT_TESTS,
            vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_READ
                | vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE,
        ),
    }
}

fn map_image_layout(state: ResourceState) -> vk::ImageLayout {
    match state.access {
        Access::TransferRead => vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
        Access::TransferWrite => vk::ImageLayout::TRANSFER_DST_OPTIMAL,
        Access::ShaderRead => vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        Access::ShaderWrite => vk::ImageLayout::GENERAL,
        Access::ColorAttachmentReadWrite => vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
        Access::DepthStencilReadWrite => vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
        Access::VertexRead | Access::IndexRead | Access::IndirectRead => vk::ImageLayout::GENERAL,
    }
}

fn map_aspect(aspect: ImageAspect) -> vk::ImageAspectFlags {
    match aspect {
        ImageAspect::Color => vk::ImageAspectFlags::COLOR,
        ImageAspect::Depth => vk::ImageAspectFlags::DEPTH,
        ImageAspect::Stencil => vk::ImageAspectFlags::STENCIL,
    }
}

fn vk_pipeline(pipeline: &dyn RhiPipeline) -> &VkPipeline {
    pipeline
        .as_any()
        .downcast_ref::<VkPipeline>()
        .expect("Vulkan command buffer received a non-Vulkan pipeline")
}

fn descriptor_set(set: &dyn RhiDescriptorSet) -> &VkDescriptorSet {
    set.as_any()
        .downcast_ref::<VkDescriptorSet>()
        .expect("Vulkan command buffer received a non-Vulkan descriptor set")
}

fn depth_stencil_attachment<'a>(
    view: &'a dyn RhiImageView,
    resolve: Option<&'a dyn RhiImageView>,
    load: LoadOp,
    store: StoreOp,
) -> vk::RenderingAttachmentInfo<'static> {
    let mut info = vk::RenderingAttachmentInfo::default()
        .image_view(image_view(view).handle())
        .image_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL)
        .load_op(map_load_op(load))
        .store_op(map_store_op(store))
        .clear_value(map_clear_value(load));
    if let Some(resolve) = resolve {
        info = info
            .resolve_mode(vk::ResolveModeFlags::SAMPLE_ZERO)
            .resolve_image_view(image_view(resolve).handle())
            .resolve_image_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);
    }
    info
}

fn map_load_op(op: LoadOp) -> vk::AttachmentLoadOp {
    match op {
        LoadOp::Load => vk::AttachmentLoadOp::LOAD,
        LoadOp::Clear(_) => vk::AttachmentLoadOp::CLEAR,
        LoadOp::DontCare => vk::AttachmentLoadOp::DONT_CARE,
    }
}

fn map_store_op(op: StoreOp) -> vk::AttachmentStoreOp {
    match op {
        StoreOp::Store => vk::AttachmentStoreOp::STORE,
        StoreOp::DontCare => vk::AttachmentStoreOp::DONT_CARE,
    }
}

fn map_clear_value(op: LoadOp) -> vk::ClearValue {
    match op {
        LoadOp::Clear(ClearValue::Color(float32)) => vk::ClearValue {
            color: vk::ClearColorValue { float32 },
        },
        LoadOp::Clear(ClearValue::DepthStencil { depth, stencil }) => vk::ClearValue {
            depth_stencil: vk::ClearDepthStencilValue { depth, stencil },
        },
        LoadOp::Load | LoadOp::DontCare => vk::ClearValue::default(),
    }
}

fn map_subresource(layers: ImageSubresourceLayers) -> vk::ImageSubresourceLayers {
    vk::ImageSubresourceLayers {
        aspect_mask: match layers.aspect {
            ImageAspect::Color => vk::ImageAspectFlags::COLOR,
            ImageAspect::Depth => vk::ImageAspectFlags::DEPTH,
            ImageAspect::Stencil => vk::ImageAspectFlags::STENCIL,
        },
        mip_level: layers.mip,
        base_array_layer: layers.base_layer,
        layer_count: layers.layer_count,
    }
}

fn map_offset(offset: [i32; 3]) -> vk::Offset3D {
    vk::Offset3D {
        x: offset[0],
        y: offset[1],
        z: offset[2],
    }
}

fn map_buffer_image_copy(region: &BufferImageCopy) -> vk::BufferImageCopy {
    vk::BufferImageCopy::default()
        .buffer_offset(region.buffer_offset)
        .image_subresource(map_subresource(region.image_subresource))
        .image_offset(map_offset(region.image_offset))
        .image_extent(vk::Extent3D {
            width: region.image_extent[0],
            height: region.image_extent[1],
            depth: region.image_extent[2],
        })
}

fn map_shader_stages(stages: ShaderStageFlags) -> vk::ShaderStageFlags {
    let mut result = vk::ShaderStageFlags::empty();
    for (rhi, vk) in [
        (ShaderStageFlags::VERTEX, vk::ShaderStageFlags::VERTEX),
        (
            ShaderStageFlags::HULL,
            vk::ShaderStageFlags::TESSELLATION_CONTROL,
        ),
        (
            ShaderStageFlags::DOMAIN,
            vk::ShaderStageFlags::TESSELLATION_EVALUATION,
        ),
        (ShaderStageFlags::GEOMETRY, vk::ShaderStageFlags::GEOMETRY),
        (ShaderStageFlags::FRAGMENT, vk::ShaderStageFlags::FRAGMENT),
        (ShaderStageFlags::COMPUTE, vk::ShaderStageFlags::COMPUTE),
    ] {
        if stages.contains(rhi) {
            result |= vk;
        }
    }
    result
}
