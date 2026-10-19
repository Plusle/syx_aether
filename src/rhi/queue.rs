#![allow(dead_code)]

use super::backend::{RhiCommandBuffer, RhiQueue};
use super::command::Command;
use super::device::DeviceInner;
use super::lifetime::{LifetimeReservation, ObjectRef};
use super::sync::{QueueCapabilities, QueueId, SyncPoint};
use super::tracking::TrackerTransaction;
use std::sync::{Arc, Mutex};

pub struct Queue {
    pub(crate) inner: Arc<QueueInner>,
}

pub(crate) struct QueueInner {
    pub(crate) id: QueueId,
    pub(crate) backend: Arc<dyn RhiQueue>,
    pub(crate) device: Arc<DeviceInner>,
    pub(crate) capabilities: QueueCapabilities,
    /// Shared by all logical queue handles mapping to the same native queue.
    pub(crate) submit_gate: Arc<Mutex<SubmitState>>,
}

pub(crate) struct SubmitState {
    pub(crate) next_value: u64,
    pub(crate) pending: Vec<PendingSubmission>,
}

pub(crate) struct PendingSubmission {
    pub(crate) completion: SyncPoint,
    pub(crate) command_buffer: Box<dyn RhiCommandBuffer>,
    pub(crate) resources: Vec<ObjectRef>,
}

pub struct Recorder<'resource> {
    pub(crate) queue: &'resource Queue,
    pub(crate) commands: Vec<Command<'resource>>,
    pub(crate) waits: Vec<SyncPoint>,
}

/// Unique mutable native command buffer, with no borrows from the recording stream.
/// Box permits mutable recording without requiring the buffer itself to be Sync.
pub struct CommandBuffer {
    pub(crate) inner: Option<Box<dyn RhiCommandBuffer>>,
    pub(crate) origin: QueueId,
    pub(crate) device: Arc<DeviceInner>,
    pub(crate) waits: Vec<SyncPoint>,
    pub(crate) transaction: Option<TrackerTransaction>,
    pub(crate) lifetime: Option<LifetimeReservation>,
}
