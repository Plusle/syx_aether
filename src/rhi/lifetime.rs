#![allow(dead_code)]

use super::backend::{
    RhiBuffer, RhiDescriptorSet, RhiDescriptorSetLayout, RhiImage, RhiImageView, RhiPipeline,
    RhiPipelineLayout, RhiQueryPool, RhiSampler, RhiShaderModule,
};
use super::sync::{DeviceId, QueueId, SyncPoint};
use super::tracking::ReadFingerprint;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ObjectId {
    pub(crate) device: DeviceId,
    pub(crate) serial: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ReservationId(pub(crate) u64);

pub(crate) struct SharedObject<T: ?Sized> {
    pub(crate) id: ObjectId,
    pub(crate) backend: Arc<T>,
    pub(crate) retire_queue: Weak<RetireQueue>,
}

/// Strong backend references protect objects referenced by views, sets, and submissions.
pub(crate) enum ObjectRef {
    Buffer(Arc<dyn RhiBuffer>),
    Image(Arc<dyn RhiImage>),
    ImageView(Arc<dyn RhiImageView>),
    Sampler(Arc<dyn RhiSampler>),
    ShaderModule(Arc<dyn RhiShaderModule>),
    DescriptorSetLayout(Arc<dyn RhiDescriptorSetLayout>),
    PipelineLayout(Arc<dyn RhiPipelineLayout>),
    Pipeline(Arc<dyn RhiPipeline>),
    DescriptorSet(Arc<dyn RhiDescriptorSet>),
    QueryPool(Arc<dyn RhiQueryPool>),
}

pub(crate) enum ReservationKind {
    Read(ReadFingerprint),
    Write,
    LifetimeOnly,
}

pub(crate) struct ObjectReservation {
    pub(crate) shared: Option<ReadFingerprint>,
    pub(crate) shared_count: u32,
    pub(crate) exclusive: bool,
}

pub(crate) struct LifetimeEntry {
    pub(crate) retired: bool,
    pub(crate) reservation: ObjectReservation,
    pub(crate) last_use: HashMap<QueueId, SyncPoint>,
}

pub(crate) struct LifetimeTracker {
    pub(crate) entries: Mutex<HashMap<ObjectId, LifetimeEntry>>,
}

pub(crate) struct ReservedObject {
    pub(crate) id: ObjectId,
    pub(crate) kind: ReservationKind,
    pub(crate) object: ObjectRef,
}

/// Committed only after successful submission; release/rollback behavior is deferred.
pub(crate) struct LifetimeReservation {
    pub(crate) id: ReservationId,
    pub(crate) tracker: Arc<LifetimeTracker>,
    pub(crate) objects: Vec<ReservedObject>,
}

pub(crate) struct DiscardedObject {
    pub(crate) id: ObjectId,
    pub(crate) object: ObjectRef,
}

pub(crate) struct RetireQueue {
    pub(crate) pending: Mutex<Vec<DiscardedObject>>,
    pub(crate) lifetime: Arc<LifetimeTracker>,
}
