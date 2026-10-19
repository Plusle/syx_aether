#![allow(dead_code)]

use super::lifetime::ObjectId;
use super::resource::{Buffer, Image};
use super::sync::{Access, QueueId, SyncPoint};
use super::types::{BufferRange, ImageAspects, ImageSubresourceRange, ResourceRange};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Canonical read-intent set, deliberately independent of resource ranges.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ReadFingerprint {
    pub(crate) intents: Vec<Access>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResourceOwner {
    Owned(QueueId),
    Released {
        source: QueueId,
        destination: QueueId,
        completion: SyncPoint,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TrackedState {
    pub(crate) access: Access,
    pub(crate) owner: ResourceOwner,
}

pub(crate) struct BufferStateRange {
    pub(crate) range: BufferRange,
    pub(crate) state: TrackedState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ImageCell {
    pub(crate) aspect: ImageAspects,
    pub(crate) mip: u32,
    pub(crate) layer: u32,
}

pub(crate) enum TrackedStates {
    Buffer(Vec<BufferStateRange>),
    Image(HashMap<ImageCell, TrackedState>),
}

pub(crate) struct ResourceTracker {
    pub(crate) committed: Mutex<HashMap<ObjectId, TrackedStates>>,
}

/// Transactions contain only touched ranges/cells, never whole-object snapshots.
pub(crate) struct StateDelta {
    pub(crate) object: ObjectId,
    pub(crate) range: ResourceRange,
    pub(crate) state: TrackedState,
}

pub(crate) struct TrackerTransaction {
    pub(crate) tracker: Arc<ResourceTracker>,
    pub(crate) deltas: Vec<StateDelta>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum BarrierKind {
    Memory,
    Release { destination: QueueId },
    Acquire { source: QueueId },
}

pub(crate) enum BarrierResource<'a> {
    Buffer(&'a Buffer, BufferRange),
    Image(&'a Image, ImageSubresourceRange),
}

/// Native barriers are recorded at a precise command-stream boundary, outside illegal scopes.
pub(crate) struct PlannedBarrier<'a> {
    pub(crate) resource: BarrierResource<'a>,
    pub(crate) before: Option<Access>,
    pub(crate) after: Access,
    pub(crate) kind: BarrierKind,
}
