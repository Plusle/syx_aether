#![allow(dead_code)]

/// A value on a backend timeline semaphore.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SyncPoint {
    timeline: u64,
    value: u64,
}

impl SyncPoint {
    pub const fn new(timeline: u64, value: u64) -> Self {
        Self { timeline, value }
    }

    pub const fn timeline(self) -> u64 {
        self.timeline
    }

    pub const fn value(self) -> u64 {
        self.value
    }
}
