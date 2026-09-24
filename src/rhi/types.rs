#![allow(dead_code)]

use std::{any::Any, fmt::Debug};

bitflags::bitflags! {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CpuAccess {
    None,
    Write,
    Read,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BufferDesc {
    pub name: String,
    pub size: u64,
    pub usage: BufferUsage,
    pub cpu_access: CpuAccess,
}

pub struct ImageDesc;
pub struct ImageViewDesc;
pub struct SamplerDesc;

pub(crate) trait RhiType: Debug + Any + Send + Sync {
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
