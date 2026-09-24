#![allow(dead_code)]

use crate::rhi::queue::QueueId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SyncPoint {
    queue: QueueId,
    timeline: u64,
    value: u64,
}

impl SyncPoint {
    pub(crate) const fn new(queue: QueueId, timeline: u64, value: u64) -> Self {
        Self {
            queue,
            timeline,
            value,
        }
    }

    pub(crate) const fn queue(self) -> QueueId {
        self.queue
    }

    pub(crate) const fn timeline(self) -> u64 {
        self.timeline
    }

    pub const fn value(self) -> u64 {
        self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_point_is_a_copyable_opaque_queue_value() {
        let queue = QueueId::allocate();
        let point = SyncPoint::new(queue, 17, 23);
        let copied = point;

        assert_eq!(copied.queue(), queue);
        assert_eq!(copied.timeline(), 17);
        assert_eq!(copied.value(), 23);
        assert_eq!(point, copied);
    }
}
