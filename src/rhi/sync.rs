#![allow(dead_code)]

use super::types::{QueueKind, ShaderStageFlags};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct DeviceId(pub(crate) u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct QueueId {
    pub(crate) device: DeviceId,
    pub(crate) index: u32,
}

/// Opaque completion value. Queue identities are device-qualified.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SyncPoint {
    pub(crate) queue: QueueId,
    pub(crate) value: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FrameIndex(pub u32);

/// Host visibility is handled by buffer read/write operations, outside this enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Access {
    VertexRead,
    IndexRead,
    IndirectRead,
    UniformRead(ShaderStageFlags),
    StorageRead(ShaderStageFlags),
    StorageWrite(ShaderStageFlags),
    SampledRead(ShaderStageFlags),
    StorageImageRead(ShaderStageFlags),
    StorageImageWrite(ShaderStageFlags),
    TransferRead,
    TransferWrite,
    ColorAttachmentReadWrite,
    InputAttachmentRead,
    DepthStencilReadWrite,
    DepthStencilSampledRead(ShaderStageFlags),
    Present,
}

/// Queue kind and supported operations are reported independently of native families.
#[derive(Clone, Copy, Debug)]
pub struct QueueCapabilities {
    pub kind: QueueKind,
    pub graphics: bool,
    pub compute: bool,
    pub transfer: bool,
    pub timestamp_valid_bits: u32,
}
