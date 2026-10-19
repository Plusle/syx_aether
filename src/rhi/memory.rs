#![allow(dead_code)]

use super::descriptor::DescriptorPool;
use super::resource::{Buffer, BufferSlice};
use super::swapchain::PresentToken;
use super::sync::{FrameIndex, SyncPoint};
use std::collections::VecDeque;
use std::sync::Mutex;

#[derive(Clone, Debug)]
pub struct FrameAllocatorDesc {
    pub bytes_per_frame: u64,
    pub frames_in_flight: u32,
}

pub struct FrameAllocator {
    pub(crate) frame: FrameIndex,
    pub(crate) buffer: Buffer,
    pub(crate) cursor: u64,
    pub(crate) completion: Option<SyncPoint>,
}

#[derive(Clone, Debug)]
pub struct StagingRingDesc {
    pub block_size: u64,
    pub block_count: u32,
}

pub(crate) struct StagingRing {
    pub(crate) blocks: Vec<StagingBlock>,
    pub(crate) current: usize,
}

pub(crate) struct StagingBlock {
    pub(crate) buffer: Buffer,
    pub(crate) cursor: u64,
    pub(crate) in_flight: VecDeque<StagingAllocation>,
}

pub(crate) struct StagingAllocation {
    pub(crate) slice: BufferSlice,
    /// None until the caller explicitly submits the recorded upload.
    pub(crate) completion: Option<SyncPoint>,
}

pub(crate) struct FrameResources {
    pub(crate) slots: Mutex<Vec<FrameSlot>>,
    pub(crate) staging: Mutex<StagingRing>,
}

pub(crate) struct FrameSlot {
    pub(crate) index: FrameIndex,
    pub(crate) allocator: FrameAllocator,
    pub(crate) descriptors: DescriptorPool,
    pub(crate) present: Option<PresentToken>,
    pub(crate) completion: Option<SyncPoint>,
}
