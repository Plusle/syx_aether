#![allow(dead_code)]

use std::{any::Any, fmt::Debug};

bitflags::bitflags! {
    /// Operations for which a buffer may be used.
    ///
    /// Backends may need this at creation time even when another backend does
    /// not. In particular, Vulkan requires a nonempty usage mask.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct BufferUsage: u32 {
        const TRANSFER_SRC  = 1 << 0;
        const TRANSFER_DST  = 1 << 1;
        const VERTEX        = 1 << 2;
        const INDEX         = 1 << 3;
        const UNIFORM       = 1 << 4;
        const STORAGE       = 1 << 5;
        const INDIRECT      = 1 << 6;
        const UNIFORM_TEXEL = 1 << 7;
        const STORAGE_TEXEL = 1 << 8;
        const DEVICE_ADDRESS = 1 << 9;
    }
}

/// CPU access required for a buffer.
///
/// Backends translate this intent into their allocator's memory-location or
/// heap selection. It does not describe the physical location of the memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CpuAccess {
    /// No CPU access. The backend should prefer GPU-local memory.
    None,
    /// CPU write access for uploading data to the GPU.
    Write,
    /// CPU read access for retrieving data produced by the GPU.
    Read,
}

/// Backend-neutral inputs required to create a buffer allocation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BufferDesc {
    /// Human-readable name used for diagnostics and allocator tracking.
    pub name: String,
    /// Logical buffer size in bytes. Must be greater than zero.
    pub size: u64,
    /// Every operation the buffer will participate in. Must not be empty.
    pub usage: BufferUsage,
    /// CPU mapping requirements for the allocation.
    pub cpu_access: CpuAccess,
}

pub struct ImageDesc;
pub struct ImageViewDesc;
pub struct SamplerDesc;

/// Common bounds for values crossing the backend-neutral RHI boundary.
///
/// Every concrete type satisfying these bounds automatically implements this
/// trait. `as_any` remains object-safe, so it also works through `dyn Rhi*`
/// trait objects.
pub trait RhiType: Debug + Any + Send + Sync {
    fn as_any(&self) -> &dyn Any;
}

impl<T> RhiType for T
where
    T: Debug + Any + Send + Sync,
{
    fn as_any(&self) -> &dyn Any {
        self
    }
}
