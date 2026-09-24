#![allow(dead_code)]

use crate::rhi::{
    command::{
        record_planned, BufferImageCopy, Command, ImageCopyRegion, IndexType, Rect, RenderingDesc,
        RhiCommandBuffer, Viewport,
    },
    descriptor::DescriptorSet,
    error::{RhiError, RhiResult},
    lifetime::{LifetimeReservation, LifetimeTracker},
    pipeline::Pipeline,
    resource::{Buffer, Image, ResourceTracker, TrackerTransaction},
    shader::ShaderStageFlags,
    sync::SyncPoint,
    types::RhiType,
};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex, MutexGuard, TryLockError,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QueueKind {
    Graphics,
    Transfer,
}

#[derive(Clone)]
pub struct Queue {
    inner: Arc<dyn RhiQueue>,
}

impl std::fmt::Debug for Queue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Queue")
            .field("kind", &self.inner.kind())
            .finish()
    }
}

impl Queue {
    pub(crate) fn new(inner: Arc<dyn RhiQueue>) -> Self {
        Self { inner }
    }

    pub fn kind(&self) -> QueueKind {
        self.inner.kind()
    }

    pub fn recorder<'queue, 'command>(&'queue self) -> RhiResult<Recorder<'queue, 'command>> {
        recorder(self.inner.as_ref())
    }

    pub fn try_recorder<'queue, 'command>(
        &'queue self,
    ) -> RhiResult<Option<Recorder<'queue, 'command>>> {
        try_recorder(self.inner.as_ref())
    }

    pub fn submit(&self, recorder: BuiltRecorder<'_>) -> RhiResult<SyncPoint> {
        submit(self.inner.as_ref(), recorder)
    }

    pub fn force_flush(&self) -> RhiResult<()> {
        force_flush(self.inner.as_ref())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct QueueId(u64);

impl QueueId {
    pub(crate) fn allocate() -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        Self(NEXT_ID.fetch_add(1, Ordering::Relaxed))
    }
}

pub(crate) trait RhiQueue: RhiType {
    fn kind(&self) -> QueueKind;
    fn id(&self) -> QueueId;
    fn gate(&self) -> &Mutex<()>;
    fn tracker(&self) -> &Arc<ResourceTracker>;
    fn lifetime(&self) -> &Arc<LifetimeTracker>;
    fn resolve_queue(&self, kind: QueueKind) -> RhiResult<QueueId>;
    fn validate_commands(&self, commands: &[Command<'_>]) -> RhiResult<()>;
    fn allocate(&self) -> RhiResult<Arc<dyn RhiCommandBuffer>>;
    fn recycle_unused(&self, command_buffer: Arc<dyn RhiCommandBuffer>);
    fn submit_buffer(
        &self,
        command_buffer: Arc<dyn RhiCommandBuffer>,
        waits: &[SyncPoint],
    ) -> RhiResult<SyncPoint>;
    fn force_flush_backend(&self) -> RhiResult<()>;
}

pub(crate) fn recorder<'queue, 'command>(
    queue: &'queue dyn RhiQueue,
) -> RhiResult<Recorder<'queue, 'command>> {
    let lease = queue
        .gate()
        .lock()
        .map_err(|_| RhiError::BackendError("queue lease poisoned".into()))?;
    allocate_recorder(queue, lease)
}

pub(crate) fn try_recorder<'queue, 'command>(
    queue: &'queue dyn RhiQueue,
) -> RhiResult<Option<Recorder<'queue, 'command>>> {
    let lease = match queue.gate().try_lock() {
        Ok(lease) => lease,
        Err(TryLockError::WouldBlock) => return Ok(None),
        Err(TryLockError::Poisoned(_)) => {
            return Err(RhiError::BackendError("queue lease poisoned".into()));
        }
    };
    allocate_recorder(queue, lease).map(Some)
}

fn allocate_recorder<'queue, 'command>(
    queue: &'queue dyn RhiQueue,
    lease: MutexGuard<'queue, ()>,
) -> RhiResult<Recorder<'queue, 'command>> {
    let command_buffer = queue.allocate()?;
    debug_assert_eq!(Arc::strong_count(&command_buffer), 1);
    debug_assert_eq!(Arc::weak_count(&command_buffer), 0);
    Ok(Recorder {
        queue,
        lease: Some(lease),
        command_buffer: Some(command_buffer),
        commands: Vec::new(),
    })
}

pub(crate) fn submit(
    queue: &dyn RhiQueue,
    mut recorder: BuiltRecorder<'_>,
) -> RhiResult<SyncPoint> {
    if queue.id() != recorder.queue.id() {
        return Err(RhiError::InvalidCommand(
            "a recorder can only be submitted to its originating queue".into(),
        ));
    }
    let command_buffer = recorder
        .command_buffer
        .take()
        .expect("built recorder owns a command buffer");
    let transaction = recorder
        .transaction
        .take()
        .expect("built recorder owns a tracker transaction");
    let lifetime = recorder
        .lifetime
        .take()
        .expect("built recorder owns a lifetime reservation");
    let point = queue.submit_buffer(command_buffer, &transaction.waits)?;
    transaction.commit();
    lifetime.commit(point);
    Ok(point)
}

pub(crate) fn force_flush(queue: &dyn RhiQueue) -> RhiResult<()> {
    let _lease = queue
        .gate()
        .lock()
        .map_err(|_| RhiError::BackendError("queue lease poisoned".into()))?;
    queue.force_flush_backend()
}

pub struct Recorder<'queue, 'command> {
    queue: &'queue dyn RhiQueue,
    // Kept for its lifetime; submit deliberately does not lock the queue again.
    lease: Option<MutexGuard<'queue, ()>>,
    command_buffer: Option<Arc<dyn RhiCommandBuffer>>,
    commands: Vec<Command<'command>>,
}

impl std::fmt::Debug for Recorder<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Recorder")
            .field("queue", &self.queue.id())
            .field("command_count", &self.commands.len())
            .finish()
    }
}

impl<'queue, 'command> Recorder<'queue, 'command> {
    pub(crate) fn push(&mut self, command: Command<'command>) {
        self.commands.push(command);
    }

    pub(crate) fn extend(&mut self, commands: impl IntoIterator<Item=Command<'command>>) {
        self.commands.extend(commands);
    }

    fn command(mut self, command: Command<'command>) -> Self {
        self.push(command);
        self
    }

    pub fn begin_rendering(self, desc: RenderingDesc<'command>) -> Self {
        self.command(Command::BeginRendering(desc))
    }

    pub fn end_rendering(self) -> Self {
        self.command(Command::EndRendering)
    }

    pub fn draw(
        self,
        vertex_count: u32,
        instance_count: u32,
        first_vertex: u32,
        first_instance: u32,
    ) -> Self {
        self.command(Command::Draw {
            vertex_count,
            instance_count,
            first_vertex,
            first_instance,
        })
    }

    pub fn draw_indexed(
        self,
        index_count: u32,
        instance_count: u32,
        first_index: u32,
        vertex_offset: i32,
        first_instance: u32,
    ) -> Self {
        self.command(Command::DrawIndexed {
            index_count,
            instance_count,
            first_index,
            vertex_offset,
            first_instance,
        })
    }

    pub fn draw_indirect(
        self,
        buffer: &'command Buffer,
        offset: u64,
        draw_count: u32,
        stride: u32,
    ) -> Self {
        self.command(Command::DrawIndirect {
            buffer,
            offset,
            draw_count,
            stride,
        })
    }

    pub fn draw_indexed_indirect(
        self,
        buffer: &'command Buffer,
        offset: u64,
        draw_count: u32,
        stride: u32,
    ) -> Self {
        self.command(Command::DrawIndexedIndirect {
            buffer,
            offset,
            draw_count,
            stride,
        })
    }

    pub fn dispatch(self, group_count: [u32; 3]) -> Self {
        self.command(Command::Dispatch(group_count))
    }

    pub fn dispatch_indirect(self, buffer: &'command Buffer, offset: u64) -> Self {
        self.command(Command::DispatchIndirect { buffer, offset })
    }

    pub fn copy_buffer(
        self,
        src: &'command Buffer,
        src_offset: u64,
        dst: &'command Buffer,
        dst_offset: u64,
        size: u64,
    ) -> Self {
        self.command(Command::CopyBuffer {
            src,
            src_offset,
            dst,
            dst_offset,
            size,
        })
    }

    pub fn copy_buffer_to_image(
        self,
        src: &'command Buffer,
        dst: &'command Image,
        region: BufferImageCopy,
    ) -> Self {
        self.command(Command::CopyBufferToImage { src, dst, region })
    }

    pub fn copy_image_to_buffer(
        self,
        src: &'command Image,
        dst: &'command Buffer,
        region: BufferImageCopy,
    ) -> Self {
        self.command(Command::CopyImageToBuffer { src, dst, region })
    }

    pub fn copy_image(
        self,
        src: &'command Image,
        src_region: ImageCopyRegion,
        dst: &'command Image,
        dst_region: ImageCopyRegion,
        extent: [u32; 3],
    ) -> Self {
        self.command(Command::CopyImage {
            src,
            src_region,
            dst,
            dst_region,
            extent,
        })
    }

    pub fn release_buffers(self, buffers: &'command [&'command Buffer], target: QueueKind) -> Self {
        self.command(Command::ReleaseBuffers { buffers, target })
    }

    pub fn release_images(self, images: &'command [&'command Image], target: QueueKind) -> Self {
        self.command(Command::ReleaseImages { images, target })
    }

    pub fn bind_pipeline(self, pipeline: &'command Pipeline) -> Self {
        self.command(Command::BindPipeline(pipeline))
    }

    pub fn bind_vertex_buffers(
        self,
        first: u32,
        buffers: &'command [(&'command Buffer, u64)],
    ) -> Self {
        self.command(Command::BindVertexBuffers { first, buffers })
    }

    pub fn bind_index_buffer(
        self,
        buffer: &'command Buffer,
        offset: u64,
        index_type: IndexType,
    ) -> Self {
        self.command(Command::BindIndexBuffer {
            buffer,
            offset,
            index_type,
        })
    }

    pub fn bind_descriptor_sets(
        self,
        pipeline: &'command Pipeline,
        first_set: u32,
        sets: &'command [&'command DescriptorSet],
    ) -> Self {
        self.command(Command::BindDescriptorSets {
            pipeline,
            first_set,
            sets,
        })
    }

    pub fn push_constants(
        self,
        pipeline: &'command Pipeline,
        stages: ShaderStageFlags,
        offset: u32,
        data: &'command [u8],
    ) -> Self {
        self.command(Command::PushConstants {
            pipeline,
            stages,
            offset,
            data,
        })
    }

    pub fn set_viewport(self, viewport: Viewport) -> Self {
        self.command(Command::SetViewport(viewport))
    }

    pub fn set_scissor(self, scissor: Rect) -> Self {
        self.command(Command::SetScissor(scissor))
    }

    pub fn set_blend_constants(self, constants: [f32; 4]) -> Self {
        self.command(Command::SetBlendConstants(constants))
    }

    pub fn set_stencil_reference(self, reference: u32) -> Self {
        self.command(Command::SetStencilReference(reference))
    }

    pub fn begin_debug_label(self, name: &'command str) -> Self {
        self.command(Command::BeginDebugLabel(name))
    }

    pub fn end_debug_label(self) -> Self {
        self.command(Command::EndDebugLabel)
    }

    pub fn insert_debug_label(self, name: &'command str) -> Self {
        self.command(Command::InsertDebugLabel(name))
    }

    pub fn build(mut self) -> RhiResult<BuiltRecorder<'queue>> {
        self.queue.validate_commands(&self.commands)?;

        let lifetime = self.queue.lifetime().reserve_commands(&self.commands)?;

        let (stream, transaction) =
            self.queue
                .tracker()
                .plan(self.queue.id(), &self.commands, &lifetime, |kind| {
                    self.queue.resolve_queue(kind)
                })?;

        let result = {
            let command_buffer = self
                .command_buffer
                .as_mut()
                .and_then(Arc::get_mut)
                .ok_or_else(|| {
                    RhiError::BackendError("recorder command buffer lost unique ownership".into())
                });
            command_buffer.and_then(|buffer| record_planned(buffer, &stream))
        };
        drop(stream);
        result?;

        Ok(BuiltRecorder {
            queue: self.queue,
            lease: self.lease.take(),
            command_buffer: self.command_buffer.take(),
            transaction: Some(transaction),
            lifetime: Some(lifetime),
        })
    }
}

impl Drop for Recorder<'_, '_> {
    fn drop(&mut self) {
        if let Some(command_buffer) = self.command_buffer.take() {
            self.queue.recycle_unused(command_buffer);
        }
        // Drop the lease last, after all queue-local state is restored.
        self.lease.take();
    }
}

pub struct BuiltRecorder<'queue> {
    queue: &'queue dyn RhiQueue,
    lease: Option<MutexGuard<'queue, ()>>,
    command_buffer: Option<Arc<dyn RhiCommandBuffer>>,
    transaction: Option<TrackerTransaction>,
    lifetime: Option<LifetimeReservation>,
}

impl std::fmt::Debug for BuiltRecorder<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BuiltRecorder")
            .field("queue", &self.queue.id())
            .finish()
    }
}

impl Drop for BuiltRecorder<'_> {
    fn drop(&mut self) {
        self.transaction.take();
        self.lifetime.take();
        if let Some(command_buffer) = self.command_buffer.take() {
            self.queue.recycle_unused(command_buffer);
        }
        self.lease.take();
    }
}

pub(crate) fn validate_queue_capabilities(
    kind: QueueKind,
    commands: &[Command<'_>],
) -> RhiResult<()> {
    if kind == QueueKind::Graphics {
        return Ok(());
    }
    if let Some(command) = commands.iter().find(|command| {
        !matches!(
            command,
            Command::CopyBuffer { .. }
                | Command::CopyBufferToImage { .. }
                | Command::CopyImageToBuffer { .. }
                | Command::CopyImage { .. }
                | Command::ReleaseBuffers { .. }
                | Command::ReleaseImages { .. }
                | Command::BeginDebugLabel(_)
                | Command::EndDebugLabel
                | Command::InsertDebugLabel(_)
        )
    }) {
        return Err(RhiError::InvalidCommand(format!(
            "the transfer queue does not support {command:?}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rhi::{
        command::{BufferImageCopy, ImageCopyRegion, IndexType, Rect, RenderingDesc, Viewport},
        lifetime::{ObjectId, RhiObject},
        resource::RhiBuffer,
        shader::ShaderStageFlags,
    };
    use std::{
        sync::atomic::{AtomicBool, AtomicUsize},
        thread,
        time::Duration,
    };

    #[derive(Debug, Default)]
    struct BufferStats {
        resets: AtomicUsize,
        begins: AtomicUsize,
        finishes: AtomicUsize,
        fail_reset: AtomicBool,
    }

    #[derive(Debug)]
    struct MockCommandBuffer {
        stats: Arc<BufferStats>,
    }

    impl RhiCommandBuffer for MockCommandBuffer {
        fn reset(&mut self) -> RhiResult<()> {
            self.stats.resets.fetch_add(1, Ordering::Relaxed);
            if self.stats.fail_reset.load(Ordering::Relaxed) {
                return Err(RhiError::BackendError("mock reset failed".into()));
            }
            Ok(())
        }
        fn begin(&mut self) -> RhiResult<()> {
            self.stats.begins.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
        fn finish(&mut self) -> RhiResult<()> {
            self.stats.finishes.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
        fn barrier(&mut self, _: &[crate::rhi::resource::Transition<'_>]) {}
        fn begin_rendering(&mut self, _: &RenderingDesc<'_>) {}
        fn end_rendering(&mut self) {}
        fn draw(&mut self, _: u32, _: u32, _: u32, _: u32) {}
        fn draw_indexed(&mut self, _: u32, _: u32, _: u32, _: i32, _: u32) {}
        fn draw_indirect(&mut self, _: &Buffer, _: u64, _: u32, _: u32) {}
        fn draw_indexed_indirect(&mut self, _: &Buffer, _: u64, _: u32, _: u32) {}
        fn dispatch(&mut self, _: [u32; 3]) {}
        fn dispatch_indirect(&mut self, _: &Buffer, _: u64) {}
        fn copy_buffer(&mut self, _: &Buffer, _: u64, _: &Buffer, _: u64, _: u64) {}
        fn copy_buffer_to_image(&mut self, _: &Buffer, _: &Image, _: &BufferImageCopy) {}
        fn copy_image_to_buffer(&mut self, _: &Image, _: &Buffer, _: &BufferImageCopy) {}
        fn copy_image(
            &mut self,
            _: &Image,
            _: &ImageCopyRegion,
            _: &Image,
            _: &ImageCopyRegion,
            _: [u32; 3],
        ) {}
        fn bind_pipeline(&mut self, _: &Pipeline) {}
        fn bind_vertex_buffers(&mut self, _: u32, _: &[(&Buffer, u64)]) {}
        fn bind_index_buffer(&mut self, _: &Buffer, _: u64, _: IndexType) {}
        fn bind_descriptor_sets(&mut self, _: &Pipeline, _: u32, _: &[&DescriptorSet]) {}
        fn push_constants(&mut self, _: &Pipeline, _: ShaderStageFlags, _: u32, _: &[u8]) {}
        fn set_viewport(&mut self, _: &Viewport) {}
        fn set_scissor(&mut self, _: &Rect) {}
        fn set_blend_constants(&mut self, _: [f32; 4]) {}
        fn set_stencil_reference(&mut self, _: u32) {}
        fn begin_debug_label(&mut self, _: &str) {}
        fn end_debug_label(&mut self) {}
        fn insert_debug_label(&mut self, _: &str) {}
    }

    struct Pending {
        buffer: Arc<dyn RhiCommandBuffer>,
        value: u64,
    }

    #[derive(Default)]
    struct MockBuffers {
        unused: Vec<Arc<dyn RhiCommandBuffer>>,
        pending: Vec<Pending>,
    }

    struct MockQueue {
        id: QueueId,
        kind: QueueKind,
        timeline: u64,
        tracker: Arc<ResourceTracker>,
        lifetime: Arc<LifetimeTracker>,
        lease: Mutex<()>,
        buffers: Mutex<MockBuffers>,
        stats: Arc<BufferStats>,
        allocations: AtomicUsize,
        next_value: AtomicU64,
        completed: AtomicU64,
        fail_submit: AtomicBool,
    }

    impl MockQueue {
        fn new(kind: QueueKind, tracker: Arc<ResourceTracker>) -> Self {
            let id = QueueId::allocate();
            Self {
                id,
                kind,
                timeline: id.0,
                tracker,
                lifetime: Arc::new(LifetimeTracker::new()),
                lease: Mutex::new(()),
                buffers: Mutex::new(MockBuffers::default()),
                stats: Arc::new(BufferStats::default()),
                allocations: AtomicUsize::new(0),
                next_value: AtomicU64::new(1),
                completed: AtomicU64::new(0),
                fail_submit: AtomicBool::new(false),
            }
        }

        fn flush_completed(&self) {
            let completed = self.completed.load(Ordering::Relaxed);
            let mut buffers = self.buffers.lock().unwrap();
            let completed_count = buffers
                .pending
                .partition_point(|pending| pending.value <= completed);
            let completed_buffers: Vec<_> = buffers
                .pending
                .drain(..completed_count)
                .map(|pending| pending.buffer)
                .collect();
            buffers.unused.extend(completed_buffers);
        }

        fn counts(&self) -> (usize, usize) {
            let buffers = self.buffers.lock().unwrap();
            (buffers.unused.len(), buffers.pending.len())
        }

        fn recorder<'queue, 'command>(&'queue self) -> RhiResult<Recorder<'queue, 'command>> {
            super::recorder(self)
        }

        fn try_recorder<'queue, 'command>(
            &'queue self,
        ) -> RhiResult<Option<Recorder<'queue, 'command>>> {
            super::try_recorder(self)
        }

        fn submit(&self, recorder: BuiltRecorder<'_>) -> RhiResult<SyncPoint> {
            super::submit(self, recorder)
        }

        fn force_flush(&self) -> RhiResult<()> {
            super::force_flush(self)
        }
    }

    impl std::fmt::Debug for MockQueue {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("MockQueue").field("id", &self.id).finish()
        }
    }

    impl RhiQueue for MockQueue {
        fn kind(&self) -> QueueKind {
            self.kind
        }

        fn id(&self) -> QueueId {
            self.id
        }
        fn gate(&self) -> &Mutex<()> {
            &self.lease
        }
        fn tracker(&self) -> &Arc<ResourceTracker> {
            &self.tracker
        }
        fn lifetime(&self) -> &Arc<LifetimeTracker> {
            &self.lifetime
        }
        fn resolve_queue(&self, kind: QueueKind) -> RhiResult<QueueId> {
            if kind == self.kind {
                Ok(self.id)
            } else {
                Err(RhiError::BackendError("mock queue is unavailable".into()))
            }
        }
        fn validate_commands(&self, commands: &[Command<'_>]) -> RhiResult<()> {
            validate_queue_capabilities(self.kind, commands)?;
            for command in commands {
                if let Command::CopyBuffer { src, dst, .. } = command {
                    if !src.inner().as_any().is::<TestBuffer>()
                        || !dst.inner().as_any().is::<TestBuffer>()
                    {
                        return Err(RhiError::InvalidCommand(
                            "mock queue received a foreign buffer".into(),
                        ));
                    }
                }
            }
            Ok(())
        }
        fn allocate(&self) -> RhiResult<Arc<dyn RhiCommandBuffer>> {
            let mut buffer = self.buffers.lock().unwrap().unused.pop();
            if buffer.is_none() {
                self.flush_completed();
                buffer = self.buffers.lock().unwrap().unused.pop();
            }
            let mut buffer = buffer.unwrap_or_else(|| {
                self.allocations.fetch_add(1, Ordering::Relaxed);
                Arc::new(MockCommandBuffer {
                    stats: Arc::clone(&self.stats),
                })
            });
            Arc::get_mut(&mut buffer).unwrap().reset()?;
            Ok(buffer)
        }
        fn recycle_unused(&self, buffer: Arc<dyn RhiCommandBuffer>) {
            self.buffers.lock().unwrap().unused.push(buffer);
        }
        fn submit_buffer(
            &self,
            buffer: Arc<dyn RhiCommandBuffer>,
            _: &[SyncPoint],
        ) -> RhiResult<SyncPoint> {
            self.flush_completed();
            if self.fail_submit.load(Ordering::Relaxed) {
                self.recycle_unused(buffer);
                return Err(RhiError::BackendError("mock submit failed".into()));
            }
            let value = self.next_value.fetch_add(1, Ordering::Relaxed);
            self.buffers
                .lock()
                .unwrap()
                .pending
                .push(Pending { buffer, value });
            Ok(SyncPoint::new(self.id, self.timeline, value))
        }
        fn force_flush_backend(&self) -> RhiResult<()> {
            let mut buffers = self.buffers.lock().unwrap();
            let completed_buffers: Vec<_> = buffers
                .pending
                .drain(..)
                .map(|pending| pending.buffer)
                .collect();
            buffers.unused.extend(completed_buffers);
            Ok(())
        }
    }

    #[derive(Debug)]
    struct TestBuffer {
        id: ObjectId,
    }

    impl TestBuffer {
        #[allow(clippy::new_ret_no_self)]
        fn new() -> Buffer {
            Buffer::from_inner(Arc::new(Self {
                id: ObjectId::allocate(),
            }))
        }
    }

    impl RhiObject for TestBuffer {
        fn id(&self) -> ObjectId {
            self.id
        }
    }

    impl RhiBuffer for TestBuffer {
        fn name(&self) -> &str {
            "test"
        }
        fn size(&self) -> u64 {
            64
        }
    }

    #[derive(Debug)]
    struct ForeignBuffer {
        id: ObjectId,
    }

    impl ForeignBuffer {
        #[allow(clippy::new_ret_no_self)]
        fn new() -> Buffer {
            Buffer::from_inner(Arc::new(Self {
                id: ObjectId::allocate(),
            }))
        }
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
            64
        }
    }

    #[test]
    fn public_queue_wrapper_forwards_recording_and_submission() {
        let inner = Arc::new(MockQueue::new(
            QueueKind::Graphics,
            Arc::new(ResourceTracker::new()),
        ));
        let queue = Queue::new(inner.clone());

        assert_eq!(queue.kind(), QueueKind::Graphics);
        let built = queue
            .recorder()
            .unwrap()
            .begin_debug_label("frame")
            .end_debug_label()
            .build()
            .unwrap();
        queue.submit(built).unwrap();
        assert_eq!(inner.counts(), (0, 1));
        queue.force_flush().unwrap();
        assert_eq!(inner.counts(), (1, 0));
    }

    #[test]
    fn recorder_holds_exclusive_queue_lease() {
        let queue = Arc::new(MockQueue::new(
            QueueKind::Graphics,
            Arc::new(ResourceTracker::new()),
        ));
        let recorder = queue.recorder().unwrap();
        assert!(queue.try_recorder().unwrap().is_none());

        let (sent, received) = std::sync::mpsc::channel();
        let other = Arc::clone(&queue);
        let thread = thread::spawn(move || {
            let recorder = other.recorder().unwrap();
            sent.send(()).unwrap();
            drop(recorder);
        });
        assert!(received.recv_timeout(Duration::from_millis(25)).is_err());
        drop(recorder);
        received.recv_timeout(Duration::from_secs(1)).unwrap();
        thread.join().unwrap();
    }

    #[test]
    fn reset_failure_discards_the_slot_and_next_allocate_is_new() {
        let queue = MockQueue::new(QueueKind::Graphics, Arc::new(ResourceTracker::new()));
        queue.stats.fail_reset.store(true, Ordering::Relaxed);
        assert!(queue.recorder().is_err());
        assert_eq!(queue.counts(), (0, 0));
        assert_eq!(queue.allocations.load(Ordering::Relaxed), 1);

        queue.stats.fail_reset.store(false, Ordering::Relaxed);
        let recorder = queue.recorder().unwrap();
        assert_eq!(queue.allocations.load(Ordering::Relaxed), 2);
        drop(recorder);
    }

    #[test]
    fn validation_failure_does_not_begin_recording_or_poison_the_lease() {
        let queue = MockQueue::new(QueueKind::Graphics, Arc::new(ResourceTracker::new()));
        let foreign = ForeignBuffer::new();
        let destination = TestBuffer::new();
        let mut recorder = queue.recorder().unwrap();
        recorder.push(Command::CopyBuffer {
            src: &foreign,
            src_offset: 0,
            dst: &destination,
            dst_offset: 0,
            size: 64,
        });
        assert!(matches!(recorder.build(), Err(RhiError::InvalidCommand(_))));
        assert_eq!(queue.stats.begins.load(Ordering::Relaxed), 0);

        let recorder = queue.recorder().unwrap();
        drop(recorder);
    }

    #[test]
    fn transfer_queue_rejects_draw_and_dispatch_commands() {
        let queue = MockQueue::new(QueueKind::Transfer, Arc::new(ResourceTracker::new()));
        let mut draw = queue.recorder().unwrap();
        draw.push(Command::Draw {
            vertex_count: 3,
            instance_count: 1,
            first_vertex: 0,
            first_instance: 0,
        });
        assert!(matches!(draw.build(), Err(RhiError::InvalidCommand(_))));

        let mut dispatch = queue.recorder().unwrap();
        dispatch.push(Command::Dispatch([1, 1, 1]));
        assert!(matches!(dispatch.build(), Err(RhiError::InvalidCommand(_))));
    }

    #[test]
    fn fluent_chain_builds_and_submits() {
        let queue = MockQueue::new(QueueKind::Transfer, Arc::new(ResourceTracker::new()));
        let source = TestBuffer::new();
        let destination = TestBuffer::new();
        let built = queue
            .recorder()
            .unwrap()
            .begin_debug_label("upload")
            .copy_buffer(&source, 0, &destination, 0, 64)
            .insert_debug_label("copied")
            .end_debug_label()
            .build()
            .unwrap();
        queue.submit(built).unwrap();
        assert_eq!(queue.counts(), (0, 1));
    }

    #[test]
    fn push_extend_and_fluent_commands_can_be_mixed() {
        let queue = MockQueue::new(QueueKind::Transfer, Arc::new(ResourceTracker::new()));
        let mut recorder = queue.recorder().unwrap();
        recorder.push(Command::BeginDebugLabel("outer"));
        recorder.extend([Command::InsertDebugLabel("middle"), Command::EndDebugLabel]);
        let built = recorder.insert_debug_label("after").build().unwrap();
        drop(built);
        assert_eq!(queue.counts(), (1, 0));
    }

    #[test]
    fn built_recorder_no_longer_borrows_command_inputs() {
        let queue = MockQueue::new(QueueKind::Transfer, Arc::new(ResourceTracker::new()));
        let built = {
            let source = TestBuffer::new();
            let destination = TestBuffer::new();
            queue
                .recorder()
                .unwrap()
                .copy_buffer(&source, 0, &destination, 0, 64)
                .build()
                .unwrap()
        };
        queue.submit(built).unwrap();
    }

    #[test]
    fn dropping_built_recorder_rolls_back_and_recycles() {
        let queue = MockQueue::new(QueueKind::Graphics, Arc::new(ResourceTracker::new()));
        let built = queue.recorder().unwrap().build().unwrap();
        assert!(queue.try_recorder().unwrap().is_none());
        drop(built);
        assert_eq!(queue.counts(), (1, 0));
        assert!(queue.try_recorder().unwrap().is_some());
    }

    #[test]
    fn dropped_and_failed_recorders_return_the_buffer_to_unused() {
        let queue = MockQueue::new(QueueKind::Graphics, Arc::new(ResourceTracker::new()));
        drop(queue.recorder().unwrap());
        assert_eq!(queue.counts(), (1, 0));

        let mut recorder = queue.recorder().unwrap();
        recorder.push(Command::EndRendering);
        assert!(recorder.build().is_err());
        assert_eq!(queue.counts(), (1, 0));
    }

    #[test]
    fn submit_commits_only_on_success_and_recycles_failures() {
        let tracker = Arc::new(ResourceTracker::new());
        let queue = MockQueue::new(QueueKind::Graphics, Arc::clone(&tracker));
        let source = TestBuffer::new();
        let destination = TestBuffer::new();

        queue.fail_submit.store(true, Ordering::Relaxed);
        let mut recorder = queue.recorder().unwrap();
        recorder.push(Command::CopyBuffer {
            src: &source,
            src_offset: 0,
            dst: &destination,
            dst_offset: 0,
            size: 64,
        });
        let built = recorder.build().unwrap();
        assert!(queue.submit(built).is_err());
        assert!(queue.lifetime.last_uses(destination.id()).is_empty());
        assert_eq!(queue.counts(), (1, 0));

        queue.fail_submit.store(false, Ordering::Relaxed);
        let mut recorder = queue.recorder().unwrap();
        recorder.push(Command::CopyBuffer {
            src: &source,
            src_offset: 0,
            dst: &destination,
            dst_offset: 0,
            size: 64,
        });
        let built = recorder.build().unwrap();
        let point = queue.submit(built).unwrap();
        assert_eq!(queue.lifetime.last_uses(destination.id()), vec![point]);
        assert_eq!(queue.counts(), (0, 1));
    }

    #[test]
    fn completed_pending_buffers_are_reused_before_allocating() {
        let queue = MockQueue::new(QueueKind::Graphics, Arc::new(ResourceTracker::new()));
        let recorder = queue.recorder().unwrap();
        let built = recorder.build().unwrap();
        let point = queue.submit(built).unwrap();
        assert_eq!(queue.allocations.load(Ordering::Relaxed), 1);

        queue.completed.store(point.value(), Ordering::Relaxed);
        let recorder = queue.recorder().unwrap();
        assert_eq!(queue.allocations.load(Ordering::Relaxed), 1);
        drop(recorder);
    }

    #[test]
    fn incomplete_pending_buffers_are_not_reused() {
        let queue = MockQueue::new(QueueKind::Graphics, Arc::new(ResourceTracker::new()));
        let first = queue.recorder().unwrap();
        let built = first.build().unwrap();
        queue.submit(built).unwrap();

        let second = queue.recorder().unwrap();
        assert_eq!(queue.allocations.load(Ordering::Relaxed), 2);
        assert_eq!(queue.counts(), (0, 1));
        drop(second);
    }

    #[test]
    fn force_flush_recycles_pending_without_destroying_buffers() {
        let queue = MockQueue::new(QueueKind::Graphics, Arc::new(ResourceTracker::new()));
        let recorder = queue.recorder().unwrap();
        let built = recorder.build().unwrap();
        queue.submit(built).unwrap();
        assert_eq!(queue.counts(), (0, 1));
        queue.force_flush().unwrap();
        assert_eq!(queue.counts(), (1, 0));
    }

    #[test]
    fn recorder_cannot_be_submitted_to_another_queue() {
        let tracker = Arc::new(ResourceTracker::new());
        let source = MockQueue::new(QueueKind::Graphics, Arc::clone(&tracker));
        let target = MockQueue::new(QueueKind::Transfer, tracker);
        let recorder = source.recorder().unwrap();
        let built = recorder.build().unwrap();
        assert!(target.submit(built).is_err());
        assert_eq!(source.counts(), (1, 0));
        assert_eq!(target.counts(), (0, 0));
    }
}
