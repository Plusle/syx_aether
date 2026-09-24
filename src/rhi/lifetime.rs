#![allow(dead_code)]

use crate::rhi::{
    backend::{backend, RhiBackend},
    command::Command,
    descriptor::RhiDescriptorSet,
    error::{RhiError, RhiResult},
    pipeline::RhiPipeline,
    resource::{RhiBuffer, RhiImage, RhiImageView, RhiSampler},
    sync::SyncPoint,
    types::RhiType,
};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct ObjectId(u64);

impl ObjectId {
    pub(crate) fn new() -> Self {
        Self::allocate()
    }

    pub(crate) fn allocate() -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        Self(NEXT_ID.fetch_add(1, Ordering::Relaxed))
    }

    pub(crate) const fn get(self) -> u64 {
        self.0
    }
}

impl Default for ObjectId {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) trait RhiObject: RhiType {
    fn id(&self) -> ObjectId;
}

#[derive(Debug)]
pub(crate) enum DiscardedObject {
    Buffer(Arc<dyn RhiBuffer>),
    Image(Arc<dyn RhiImage>),
    ImageView(Arc<dyn RhiImageView>),
    Sampler(Arc<dyn RhiSampler>),
    Pipeline(Arc<dyn RhiPipeline>),
    Descriptor(Arc<dyn RhiDescriptorSet>),
}

impl DiscardedObject {
    fn id(&self) -> ObjectId {
        match self {
            Self::Buffer(object) => object.id(),
            Self::Image(object) => object.id(),
            Self::ImageView(object) => object.id(),
            Self::Sampler(object) => object.id(),
            Self::Pipeline(object) => object.id(),
            Self::Descriptor(object) => object.id(),
        }
    }

    fn destroy(self, backend: &dyn RhiBackend) {
        match self {
            Self::Buffer(object) => backend.destroy_buffer(object),
            Self::Image(object) => backend.destroy_image(object),
            Self::ImageView(object) => backend.destroy_image_view(object),
            Self::Sampler(object) => backend.destroy_sampler(object),
            Self::Pipeline(object) => backend.destroy_pipeline(object),
            Self::Descriptor(object) => backend.destroy_descriptor_set(object),
        }
    }

    fn is_unique(&self) -> bool {
        match self {
            Self::Buffer(object) => Arc::strong_count(object) == 1,
            Self::Image(object) => Arc::strong_count(object) == 1,
            Self::ImageView(object) => Arc::strong_count(object) == 1,
            Self::Sampler(object) => Arc::strong_count(object) == 1,
            Self::Pipeline(object) => Arc::strong_count(object) == 1,
            Self::Descriptor(object) => Arc::strong_count(object) == 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReservationId(u64);

impl ReservationId {
    fn allocate() -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        Self(NEXT_ID.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Debug, Default)]
struct LifetimeEntry {
    reservation: Option<ReservationId>,
    last_uses: HashMap<crate::rhi::queue::QueueId, SyncPoint>,
    retired: bool,
}

#[derive(Debug, Default)]
pub(crate) struct LifetimeTracker {
    objects: Mutex<HashMap<ObjectId, LifetimeEntry>>,
}

static GLOBAL_LIFETIME: OnceLock<Arc<LifetimeTracker>> = OnceLock::new();

pub(crate) fn lifetime_tracker() -> Arc<LifetimeTracker> {
    Arc::clone(GLOBAL_LIFETIME.get_or_init(|| Arc::new(LifetimeTracker::new())))
}

#[derive(Debug)]
pub(crate) struct DiscardManager {
    lifetime: Arc<LifetimeTracker>,
    pending: Mutex<Vec<DiscardedObject>>,
}

static GLOBAL_DISCARD: OnceLock<Arc<DiscardManager>> = OnceLock::new();

impl DiscardManager {
    pub(crate) fn global() -> Arc<Self> {
        Arc::clone(GLOBAL_DISCARD.get_or_init(|| Arc::new(Self::new(lifetime_tracker()))))
    }

    pub(crate) fn new(lifetime: Arc<LifetimeTracker>) -> Self {
        Self {
            lifetime,
            pending: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn retire(&self, object: DiscardedObject) {
        self.lifetime.retire(object.id());
        self.pending
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(object);
    }

    pub(crate) fn flush(&self, completed: impl Fn(SyncPoint) -> bool) -> RhiResult<()> {
        let backend = backend()?;
        let pending = std::mem::take(
            &mut *self
                .pending
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        );
        let mut ready = Vec::new();
        let mut remaining = Vec::new();
        for object in pending {
            if object.is_unique() && self.lifetime.can_destroy(object.id(), &completed) {
                ready.push(object);
            } else {
                remaining.push(object);
            }
        }
        self.pending
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .extend(remaining);

        for object in ready {
            let id = object.id();
            object.destroy(backend.inner());
            self.lifetime.forget(id);
        }
        Ok(())
    }

    pub(crate) fn destroy_all(&self) -> RhiResult<()> {
        let backend = backend()?;
        let mut pending = std::mem::take(
            &mut *self
                .pending
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        );
        while !pending.is_empty() {
            let index = pending
                .iter()
                .position(DiscardedObject::is_unique)
                .expect("discarded backend objects contain an ownership cycle");
            let object = pending.swap_remove(index);
            let id = object.id();
            object.destroy(backend.inner());
            self.lifetime.forget(id);
        }
        Ok(())
    }
}

impl LifetimeTracker {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn reserve_commands(
        self: &Arc<Self>,
        commands: &[Command<'_>],
    ) -> RhiResult<LifetimeReservation> {
        self.reserve(command_objects(commands))
    }

    pub(crate) fn reserve(
        self: &Arc<Self>,
        objects: impl IntoIterator<Item=ObjectId>,
    ) -> RhiResult<LifetimeReservation> {
        let objects: Vec<_> = objects
            .into_iter()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let id = ReservationId::allocate();
        let mut entries = self
            .objects
            .lock()
            .map_err(|_| RhiError::ResourceState("lifetime tracker poisoned".into()))?;

        for object in &objects {
            if let Some(entry) = entries.get(object) {
                if entry.retired {
                    return Err(RhiError::ResourceState(format!(
                        "object {} has already been retired",
                        object.get()
                    )));
                }
                if entry.reservation.is_some() {
                    return Err(RhiError::ResourceState(format!(
                        "object {} is already reserved by another pending submission",
                        object.get()
                    )));
                }
            }
        }

        let mut last_uses = HashMap::new();
        for object in &objects {
            let entry = entries.entry(*object).or_default();
            entry.reservation = Some(id);
            if !entry.last_uses.is_empty() {
                last_uses.insert(*object, entry.last_uses.values().copied().collect());
            }
        }
        drop(entries);

        Ok(LifetimeReservation {
            tracker: Arc::clone(self),
            id,
            objects,
            last_uses,
            committed: false,
        })
    }

    pub(crate) fn retire(&self, id: ObjectId) {
        self.objects
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .entry(id)
            .or_default()
            .retired = true;
    }

    pub(crate) fn can_destroy(&self, id: ObjectId, completed: impl Fn(SyncPoint) -> bool) -> bool {
        let objects = self
            .objects
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(object) = objects.get(&id) else {
            return false;
        };
        object.retired
            && object.reservation.is_none()
            && object.last_uses.values().copied().all(completed)
    }

    pub(crate) fn forget(&self, id: ObjectId) {
        self.objects
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&id);
    }

    #[cfg(test)]
    pub(crate) fn last_uses(&self, id: ObjectId) -> Vec<SyncPoint> {
        self.objects
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(&id)
            .map(|entry| entry.last_uses.values().copied().collect())
            .unwrap_or_default()
    }
}

#[derive(Debug)]
pub(crate) struct LifetimeReservation {
    tracker: Arc<LifetimeTracker>,
    id: ReservationId,
    objects: Vec<ObjectId>,
    last_uses: HashMap<ObjectId, Vec<SyncPoint>>,
    committed: bool,
}

impl LifetimeReservation {
    pub(crate) fn last_uses(&self, id: ObjectId) -> &[SyncPoint] {
        self.last_uses.get(&id).map(Vec::as_slice).unwrap_or(&[])
    }

    pub(crate) fn commit(mut self, point: SyncPoint) {
        let mut entries = self
            .tracker
            .objects
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        for object in &self.objects {
            if let Some(entry) = entries.get_mut(object) {
                if entry.reservation == Some(self.id) {
                    entry.last_uses.insert(point.queue(), point);
                    entry.reservation = None;
                }
            }
        }
        self.committed = true;
    }
}

impl Drop for LifetimeReservation {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        let mut entries = self
            .tracker
            .objects
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        for object in &self.objects {
            if let Some(entry) = entries.get_mut(object) {
                if entry.reservation == Some(self.id) {
                    entry.reservation = None;
                }
            }
        }
    }
}

fn command_objects(commands: &[Command<'_>]) -> Vec<ObjectId> {
    let mut objects = Vec::new();
    for command in commands {
        match command {
            Command::BeginRendering(desc) => {
                for attachment in desc.color_attachments {
                    objects.push(attachment.view.id());
                    objects.push(attachment.view.image().id());
                    if let Some(resolve) = attachment.resolve {
                        objects.push(resolve.id());
                        objects.push(resolve.image().id());
                    }
                }
                if let Some(attachment) = desc.depth_stencil {
                    objects.push(attachment.view.id());
                    objects.push(attachment.view.image().id());
                    if let Some(resolve) = attachment.resolve {
                        objects.push(resolve.id());
                        objects.push(resolve.image().id());
                    }
                }
            }
            Command::DrawIndirect { buffer, .. }
            | Command::DrawIndexedIndirect { buffer, .. }
            | Command::DispatchIndirect { buffer, .. }
            | Command::BindIndexBuffer { buffer, .. } => objects.push(buffer.id()),
            Command::CopyBuffer { src, dst, .. } => {
                objects.push(src.id());
                objects.push(dst.id());
            }
            Command::CopyBufferToImage { src, dst, .. } => {
                objects.push(src.id());
                objects.push(dst.id());
            }
            Command::CopyImageToBuffer { src, dst, .. } => {
                objects.push(src.id());
                objects.push(dst.id());
            }
            Command::CopyImage { src, dst, .. } => {
                objects.push(src.id());
                objects.push(dst.id());
            }
            Command::ReleaseBuffers { buffers, .. } => {
                objects.extend(buffers.iter().map(|buffer| buffer.id()));
            }
            Command::ReleaseImages { images, .. } => {
                objects.extend(images.iter().map(|image| image.id()));
            }
            Command::BindPipeline(pipeline) => objects.push(pipeline.id()),
            Command::BindVertexBuffers { buffers, .. } => {
                objects.extend(buffers.iter().map(|(buffer, _)| buffer.id()));
            }
            Command::BindDescriptorSets { pipeline, sets, .. } => {
                objects.push(pipeline.id());
                objects.extend(sets.iter().map(|set| set.id()));
            }
            Command::PushConstants { pipeline, .. } => objects.push(pipeline.id()),
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
    objects
}
