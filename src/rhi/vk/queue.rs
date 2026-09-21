#![allow(dead_code)]

use crate::rhi::{
    command::{Command, RhiCommandBuffer},
    descriptor::{DescriptorSet, RhiDescriptorSet},
    error::{RhiError, RhiResult},
    lifetime::LifetimeTracker,
    pipeline::{Pipeline, RhiPipeline},
    queue::{validate_queue_capabilities, QueueId, QueueKind, RhiQueue},
    resource::{Buffer, Image, ImageView, ResourceTracker, RhiBuffer, RhiImage, RhiImageView},
    sync::SyncPoint,
    vk::{
        command::VkCommandBuffer,
        descriptor::VkDescriptorSet,
        device::Device,
        pipeline::VkPipeline,
        resource::{VkBuffer, VkImage, VkImageView},
    },
};
use ash::{vk, vk::Handle};
use std::{
    collections::HashMap,
    fmt::{Debug, Formatter},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};

#[derive(Debug, Clone, Copy)]
pub(crate) struct VkQueueInfo {
    pub(crate) id: QueueId,
    pub(crate) kind: QueueKind,
    pub(crate) family: u32,
    pub(crate) index: u32,
}

#[derive(Debug)]
pub(crate) struct VkQueueRegistry {
    by_kind: HashMap<QueueKind, VkQueueInfo>,
    family_by_id: HashMap<QueueId, u32>,
}

impl VkQueueRegistry {
    pub(crate) fn new(infos: impl IntoIterator<Item = VkQueueInfo>) -> Self {
        let mut by_kind = HashMap::new();
        let mut family_by_id = HashMap::new();
        for info in infos {
            by_kind.insert(info.kind, info);
            family_by_id.insert(info.id, info.family);
        }
        Self {
            by_kind,
            family_by_id,
        }
    }

    pub(crate) fn resolve(&self, kind: QueueKind) -> RhiResult<QueueId> {
        self.by_kind
            .get(&kind)
            .map(|info| info.id)
            .ok_or_else(|| RhiError::BackendError(format!("{kind:?} queue is unavailable")))
    }

    pub(super) fn family(&self, id: QueueId) -> u32 {
        *self
            .family_by_id
            .get(&id)
            .expect("resource tracker referenced an unknown queue")
    }
}

struct PendingCommandBuffer {
    command_buffer: Arc<dyn RhiCommandBuffer>,
    completion_value: u64,
}

#[derive(Default)]
struct CommandBuffers {
    unused: Vec<Arc<dyn RhiCommandBuffer>>,
    pending: Vec<PendingCommandBuffer>,
}

pub(crate) struct VkCommandQueue {
    ctx: Arc<Device>,
    handle: vk::Queue,
    info: VkQueueInfo,
    timeline: vk::Semaphore,
    next_value: AtomicU64,
    command_pool: vk::CommandPool,
    command_buffers: Mutex<CommandBuffers>,
    lease: Mutex<()>,
    submit_gate: Arc<Mutex<()>>,
    tracker: Arc<ResourceTracker>,
    queues: Arc<VkQueueRegistry>,
}

impl VkCommandQueue {
    pub(crate) fn new(
        ctx: Arc<Device>,
        info: VkQueueInfo,
        tracker: Arc<ResourceTracker>,
        queues: Arc<VkQueueRegistry>,
        submit_gate: Arc<Mutex<()>>,
    ) -> RhiResult<Self> {
        let handle = unsafe { ctx.device.get_device_queue(info.family, info.index) };
        let pool_info = vk::CommandPoolCreateInfo::default()
            .queue_family_index(info.family)
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
        let command_pool = unsafe { ctx.device.create_command_pool(&pool_info, None) }?;
        let mut timeline_info = vk::SemaphoreTypeCreateInfo::default()
            .semaphore_type(vk::SemaphoreType::TIMELINE)
            .initial_value(0);
        let semaphore_info = vk::SemaphoreCreateInfo::default().push_next(&mut timeline_info);
        let timeline = match unsafe { ctx.device.create_semaphore(&semaphore_info, None) } {
            Ok(timeline) => timeline,
            Err(error) => {
                unsafe { ctx.device.destroy_command_pool(command_pool, None) };
                return Err(error.into());
            }
        };

        Ok(Self {
            ctx,
            handle,
            info,
            timeline,
            next_value: AtomicU64::new(1),
            command_pool,
            command_buffers: Mutex::new(CommandBuffers::default()),
            lease: Mutex::new(()),
            submit_gate,
            tracker,
            queues,
        })
    }

    pub(crate) fn timeline(&self) -> vk::Semaphore {
        self.timeline
    }

    pub(crate) fn native_handle(&self) -> vk::Queue {
        self.handle
    }

    fn flush_completed(&self) -> RhiResult<()> {
        let completed = unsafe { self.ctx.device.get_semaphore_counter_value(self.timeline) }?;
        let mut buffers = self
            .command_buffers
            .lock()
            .map_err(|_| RhiError::BackendError("command-buffer pool poisoned".into()))?;
        let completed_count = buffers
            .pending
            .partition_point(|pending| pending.completion_value <= completed);
        let completed_buffers: Vec<_> = buffers
            .pending
            .drain(..completed_count)
            .map(|pending| pending.command_buffer)
            .collect();
        buffers.unused.extend(completed_buffers);
        Ok(())
    }

    fn allocate_new(&self) -> RhiResult<Arc<dyn RhiCommandBuffer>> {
        let info = vk::CommandBufferAllocateInfo::default()
            .command_pool(self.command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        let handle = unsafe { self.ctx.device.allocate_command_buffers(&info) }?
            .into_iter()
            .next()
            .expect("one Vulkan command buffer was requested");
        Ok(Arc::new(VkCommandBuffer::new(
            self.ctx.device.clone(),
            handle,
            Arc::clone(&self.queues),
        )))
    }

    fn submit_inner(
        &self,
        command_buffer: &Arc<dyn RhiCommandBuffer>,
        waits: &[SyncPoint],
    ) -> RhiResult<(SyncPoint, u64)> {
        self.flush_completed()?;
        let command_buffer = command_buffer
            .as_any()
            .downcast_ref::<VkCommandBuffer>()
            .ok_or_else(|| {
                RhiError::BackendError("Vulkan queue received a foreign command buffer".into())
            })?;
        let value = self.next_value.fetch_add(1, Ordering::Relaxed);

        let wait_infos: Vec<_> = waits
            .iter()
            .map(|point| {
                vk::SemaphoreSubmitInfo::default()
                    .semaphore(vk::Semaphore::from_raw(point.timeline()))
                    .value(point.value())
                    .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
            })
            .collect();
        let command_infos =
            [vk::CommandBufferSubmitInfo::default().command_buffer(command_buffer.handle())];
        let signal_infos = [vk::SemaphoreSubmitInfo::default()
            .semaphore(self.timeline)
            .value(value)
            .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)];
        let submit_infos = [vk::SubmitInfo2::default()
            .wait_semaphore_infos(&wait_infos)
            .command_buffer_infos(&command_infos)
            .signal_semaphore_infos(&signal_infos)];
        let _submit = self
            .submit_gate
            .lock()
            .map_err(|_| RhiError::BackendError("native queue submit gate poisoned".into()))?;
        unsafe {
            self.ctx
                .device
                .queue_submit2(self.handle, &submit_infos, vk::Fence::null())
        }?;
        Ok((SyncPoint::new(self.timeline.as_raw(), value), value))
    }
}

impl Debug for VkCommandQueue {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VkCommandQueue")
            .field("kind", &self.info.kind)
            .field("family", &self.info.family)
            .field("index", &self.info.index)
            .finish()
    }
}

impl RhiQueue for VkCommandQueue {
    fn kind(&self) -> QueueKind {
        self.info.kind
    }

    fn id(&self) -> QueueId {
        self.info.id
    }

    fn gate(&self) -> &Mutex<()> {
        &self.lease
    }

    fn tracker(&self) -> &Arc<ResourceTracker> {
        &self.tracker
    }

    fn lifetime(&self) -> &Arc<LifetimeTracker> {
        &self.ctx.lifetime
    }

    fn resolve_queue(&self, kind: QueueKind) -> RhiResult<QueueId> {
        self.queues.resolve(kind)
    }

    fn validate_commands(&self, commands: &[crate::rhi::command::Command<'_>]) -> RhiResult<()> {
        validate_queue_capabilities(self.info.kind, commands)?;
        validate_vk_commands(commands)
    }

    fn allocate(&self) -> RhiResult<Arc<dyn RhiCommandBuffer>> {
        let mut command_buffer = self
            .command_buffers
            .lock()
            .map_err(|_| RhiError::BackendError("command-buffer pool poisoned".into()))?
            .unused
            .pop();
        if command_buffer.is_none() {
            self.flush_completed()?;
            command_buffer = self
                .command_buffers
                .lock()
                .map_err(|_| RhiError::BackendError("command-buffer pool poisoned".into()))?
                .unused
                .pop();
        }
        let mut command_buffer = match command_buffer {
            Some(command_buffer) => command_buffer,
            None => self.allocate_new()?,
        };
        let reset = Arc::get_mut(&mut command_buffer)
            .ok_or_else(|| {
                RhiError::BackendError("pooled command buffer is not uniquely owned".into())
            })?
            .reset();
        if let Err(error) = reset {
            // A reset failure can be persistent for this native handle. Keep
            // it out of the reusable pool; the command pool still owns and
            // eventually destroys the native command buffer.
            drop(command_buffer);
            return Err(error);
        }
        Ok(command_buffer)
    }

    fn recycle_unused(&self, command_buffer: Arc<dyn RhiCommandBuffer>) {
        debug_assert_eq!(Arc::strong_count(&command_buffer), 1);
        debug_assert_eq!(Arc::weak_count(&command_buffer), 0);
        self.command_buffers
            .lock()
            .expect("command-buffer pool poisoned")
            .unused
            .push(command_buffer);
    }

    fn submit_buffer(
        &self,
        command_buffer: Arc<dyn RhiCommandBuffer>,
        waits: &[SyncPoint],
    ) -> RhiResult<SyncPoint> {
        match self.submit_inner(&command_buffer, waits) {
            Ok((point, completion_value)) => {
                self.command_buffers
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .pending
                    .push(PendingCommandBuffer {
                        command_buffer,
                        completion_value,
                    });
                Ok(point)
            }
            Err(error) => {
                self.recycle_unused(command_buffer);
                Err(error)
            }
        }
    }

    fn force_flush_backend(&self) -> RhiResult<()> {
        let _submit = self
            .submit_gate
            .lock()
            .map_err(|_| RhiError::BackendError("native queue submit gate poisoned".into()))?;
        unsafe { self.ctx.device.queue_wait_idle(self.handle) }?;
        let mut buffers = self
            .command_buffers
            .lock()
            .map_err(|_| RhiError::BackendError("command-buffer pool poisoned".into()))?;
        let completed_buffers: Vec<_> = buffers
            .pending
            .drain(..)
            .map(|pending| pending.command_buffer)
            .collect();
        buffers.unused.extend(completed_buffers);
        Ok(())
    }
}

impl Drop for VkCommandQueue {
    fn drop(&mut self) {
        let _submit = self
            .submit_gate
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        unsafe {
            let _ = self.ctx.device.queue_wait_idle(self.handle);
        }
        let buffers = self
            .command_buffers
            .get_mut()
            .unwrap_or_else(|error| error.into_inner());
        buffers.pending.clear();
        buffers.unused.clear();
        unsafe {
            self.ctx.device.destroy_semaphore(self.timeline, None);
            self.ctx
                .device
                .destroy_command_pool(self.command_pool, None);
        }
    }
}

fn validate_vk_commands(commands: &[Command<'_>]) -> RhiResult<()> {
    for command in commands {
        match command {
            Command::BeginRendering(desc) => {
                for attachment in desc.color_attachments {
                    validate_view(attachment.view)?;
                    if let Some(resolve) = attachment.resolve {
                        validate_view(resolve)?;
                    }
                }
                if let Some(attachment) = desc.depth_stencil {
                    validate_view(attachment.view)?;
                    if let Some(resolve) = attachment.resolve {
                        validate_view(resolve)?;
                    }
                }
            }
            Command::DrawIndirect { buffer, .. }
            | Command::DrawIndexedIndirect { buffer, .. }
            | Command::DispatchIndirect { buffer, .. }
            | Command::BindIndexBuffer { buffer, .. } => validate_buffer(*buffer)?,
            Command::CopyBuffer { src, dst, .. } => {
                validate_buffer(*src)?;
                validate_buffer(*dst)?;
            }
            Command::CopyBufferToImage { src, dst, .. } => {
                validate_buffer(*src)?;
                validate_image(*dst)?;
            }
            Command::CopyImageToBuffer { src, dst, .. } => {
                validate_image(*src)?;
                validate_buffer(*dst)?;
            }
            Command::CopyImage { src, dst, .. } => {
                validate_image(*src)?;
                validate_image(*dst)?;
            }
            Command::ReleaseBuffers { buffers, .. } => {
                for buffer in *buffers {
                    validate_buffer(*buffer)?;
                }
            }
            Command::ReleaseImages { images, .. } => {
                for image in *images {
                    validate_image(*image)?;
                }
            }
            Command::BindPipeline(pipeline) => validate_pipeline(*pipeline)?,
            Command::BindVertexBuffers { buffers, .. } => {
                for (buffer, _) in *buffers {
                    validate_buffer(*buffer)?;
                }
            }
            Command::BindDescriptorSets { pipeline, sets, .. } => {
                validate_pipeline(*pipeline)?;
                for set in *sets {
                    validate_descriptor_set(*set)?;
                }
            }
            Command::PushConstants { pipeline, .. } => validate_pipeline(*pipeline)?,
            Command::EndRendering
            | Command::Draw { .. }
            | Command::DrawIndexed { .. }
            | Command::Dispatch(_)
            | Command::SetViewport(_)
            | Command::SetScissor(_)
            | Command::SetBlendConstants(_)
            | Command::SetStencilReference(_)
            | Command::BeginDebugLabel(_)
            | Command::EndDebugLabel
            | Command::InsertDebugLabel(_) => {}
        }
    }
    Ok(())
}

fn validate_buffer(buffer: &dyn RhiBuffer) -> RhiResult<()> {
    let buffer = buffer
        .as_any()
        .downcast_ref::<Buffer>()
        .map(Buffer::inner)
        .unwrap_or(buffer);
    if buffer.as_any().is::<VkBuffer>() {
        Ok(())
    } else {
        Err(RhiError::InvalidCommand(
            "Vulkan queue received a foreign buffer".into(),
        ))
    }
}

fn validate_image(image: &dyn RhiImage) -> RhiResult<()> {
    let image = image
        .as_any()
        .downcast_ref::<Image>()
        .map(Image::inner)
        .unwrap_or(image);
    if image.as_any().is::<VkImage>() {
        Ok(())
    } else {
        Err(RhiError::InvalidCommand(
            "Vulkan queue received a foreign image".into(),
        ))
    }
}

fn validate_view(view: &dyn RhiImageView) -> RhiResult<()> {
    let view = view
        .as_any()
        .downcast_ref::<ImageView>()
        .map(ImageView::inner)
        .unwrap_or(view);
    if view.as_any().is::<VkImageView>() {
        Ok(())
    } else {
        Err(RhiError::InvalidCommand(
            "Vulkan queue received a foreign image view".into(),
        ))
    }
}

fn validate_pipeline(pipeline: &dyn RhiPipeline) -> RhiResult<()> {
    let pipeline = pipeline
        .as_any()
        .downcast_ref::<Pipeline>()
        .map(Pipeline::inner)
        .unwrap_or(pipeline);
    if pipeline.as_any().is::<VkPipeline>() {
        Ok(())
    } else {
        Err(RhiError::InvalidCommand(
            "Vulkan queue received a foreign pipeline".into(),
        ))
    }
}

fn validate_descriptor_set(set: &dyn RhiDescriptorSet) -> RhiResult<()> {
    let set = set
        .as_any()
        .downcast_ref::<DescriptorSet>()
        .map(DescriptorSet::inner)
        .unwrap_or(set);
    if set.as_any().is::<VkDescriptorSet>() {
        Ok(())
    } else {
        Err(RhiError::InvalidCommand(
            "Vulkan queue received a foreign descriptor set".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rhi::lifetime::{ObjectId, RhiObject};

    #[derive(Debug)]
    struct ForeignBuffer {
        id: ObjectId,
    }

    impl RhiObject for ForeignBuffer {
        fn id(&self) -> ObjectId {
            self.id
        }
    }

    impl RhiBuffer for ForeignBuffer {
        fn name(&self) -> &str {
            "foreign"
        }

        fn size(&self) -> u64 {
            16
        }
    }

    #[test]
    fn validation_rejects_foreign_buffer_before_vulkan_recording() {
        let foreign = Buffer::from_inner(Arc::new(ForeignBuffer {
            id: ObjectId::allocate(),
        }));
        let commands = [Command::CopyBuffer {
            src: &foreign,
            src_offset: 0,
            dst: &foreign,
            dst_offset: 0,
            size: 16,
        }];
        assert!(matches!(
            validate_vk_commands(&commands),
            Err(RhiError::InvalidCommand(_))
        ));
    }
}
