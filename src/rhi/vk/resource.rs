#![allow(dead_code)]

use super::device::Device;
use crate::rhi::{
    error::{RhiError, RhiResult},
    resource::{ImageSubresourceRange, ResourceId, RhiBuffer, RhiImage, RhiImageView, RhiSampler},
    types::{BufferDesc, BufferUsage, CpuAccess},
};
use ash::vk;
use std::{
    fmt::{Debug, Formatter},
    sync::Arc,
};
use vma::{Allocation, AllocationCreateFlags, AllocationCreateInfo, MemoryUsage};

pub(crate) struct VkBuffer {
    id: ResourceId,
    ctx: Arc<Device>,
    name: String,
    size: u64,
    handle: vk::Buffer,
    allocation: Option<Allocation>,
}

impl VkBuffer {
    pub(crate) fn new(ctx: Arc<Device>, desc: BufferDesc) -> RhiResult<Self> {
        if desc.size == 0 {
            return Err(RhiError::BackendError(
                "buffer size must be greater than zero".into(),
            ));
        }
        if desc.usage.is_empty() {
            return Err(RhiError::BackendError(
                "buffer usage must not be empty".into(),
            ));
        }
        if !BufferUsage::all().contains(desc.usage) {
            return Err(RhiError::BackendError(
                "buffer usage contains unknown flags".into(),
            ));
        }
        if desc.usage.contains(BufferUsage::DEVICE_ADDRESS) && !ctx.buffer_device_address {
            return Err(RhiError::BackendError(
                "buffer device address is not supported by this Vulkan device".into(),
            ));
        }

        let usage = map_buffer_usage(desc.usage);
        let create_info = vk::BufferCreateInfo::default()
            .size(desc.size)
            .usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let (memory_usage, allocation_flags) = match desc.cpu_access {
            CpuAccess::None => (
                MemoryUsage::AutoPreferDevice,
                AllocationCreateFlags::empty(),
            ),
            CpuAccess::Write => (
                MemoryUsage::AutoPreferHost,
                AllocationCreateFlags::HOST_ACCESS_SEQUENTIAL_WRITE,
            ),
            CpuAccess::Read => (
                MemoryUsage::AutoPreferHost,
                AllocationCreateFlags::HOST_ACCESS_RANDOM,
            ),
        };
        let allocation_info = AllocationCreateInfo {
            usage: memory_usage,
            flags: allocation_flags,
            ..Default::default()
        };
        let (handle, allocation) = ctx
            .allocator()
            .create_buffer(&create_info, &allocation_info)?;

        Ok(Self {
            id: ResourceId::allocate(),
            ctx,
            name: desc.name,
            size: desc.size,
            handle,
            allocation: Some(allocation),
        })
    }

    pub(crate) fn handle(&self) -> vk::Buffer {
        self.handle
    }
}

impl Debug for VkBuffer {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VkBuffer")
            .field("name", &self.name)
            .field("handle", &self.handle)
            .finish()
    }
}

impl RhiBuffer for VkBuffer {
    fn id(&self) -> ResourceId {
        self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn size(&self) -> u64 {
        self.size
    }
}

impl Drop for VkBuffer {
    fn drop(&mut self) {
        if let Some(allocation) = self.allocation.take() {
            self.ctx.defer_buffer(self.id, self.handle, allocation);
        }
    }
}

fn map_buffer_usage(usage: BufferUsage) -> vk::BufferUsageFlags {
    let mut result = vk::BufferUsageFlags::empty();
    for (rhi, vk) in [
        (
            BufferUsage::TRANSFER_SRC,
            vk::BufferUsageFlags::TRANSFER_SRC,
        ),
        (
            BufferUsage::TRANSFER_DST,
            vk::BufferUsageFlags::TRANSFER_DST,
        ),
        (BufferUsage::VERTEX, vk::BufferUsageFlags::VERTEX_BUFFER),
        (BufferUsage::INDEX, vk::BufferUsageFlags::INDEX_BUFFER),
        (BufferUsage::UNIFORM, vk::BufferUsageFlags::UNIFORM_BUFFER),
        (BufferUsage::STORAGE, vk::BufferUsageFlags::STORAGE_BUFFER),
        (BufferUsage::INDIRECT, vk::BufferUsageFlags::INDIRECT_BUFFER),
        (
            BufferUsage::UNIFORM_TEXEL,
            vk::BufferUsageFlags::UNIFORM_TEXEL_BUFFER,
        ),
        (
            BufferUsage::STORAGE_TEXEL,
            vk::BufferUsageFlags::STORAGE_TEXEL_BUFFER,
        ),
        (
            BufferUsage::DEVICE_ADDRESS,
            vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS,
        ),
    ] {
        if usage.contains(rhi) {
            result |= vk;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::map_buffer_usage;
    use crate::rhi::types::BufferUsage;
    use ash::vk;

    #[test]
    fn buffer_usage_maps_to_vulkan() {
        let all = BufferUsage::all();
        let expected = vk::BufferUsageFlags::TRANSFER_SRC
            | vk::BufferUsageFlags::TRANSFER_DST
            | vk::BufferUsageFlags::VERTEX_BUFFER
            | vk::BufferUsageFlags::INDEX_BUFFER
            | vk::BufferUsageFlags::UNIFORM_BUFFER
            | vk::BufferUsageFlags::STORAGE_BUFFER
            | vk::BufferUsageFlags::INDIRECT_BUFFER
            | vk::BufferUsageFlags::UNIFORM_TEXEL_BUFFER
            | vk::BufferUsageFlags::STORAGE_TEXEL_BUFFER
            | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS;

        assert_eq!(map_buffer_usage(all), expected);
    }
}

#[derive(Debug)]
pub(crate) struct VkImage {
    pub(crate) id: ResourceId,
    pub(crate) handle: vk::Image,
}

impl VkImage {
    pub(crate) fn handle(&self) -> vk::Image {
        self.handle
    }
}

impl RhiImage for VkImage {
    fn id(&self) -> ResourceId {
        self.id
    }
}

#[derive(Debug)]
pub(crate) struct VkSampler {}

impl RhiSampler for VkSampler {}

#[derive(Debug)]
pub(crate) struct VkImageView {
    pub(crate) image_id: ResourceId,
    pub(crate) image_handle: vk::Image,
    pub(crate) range: ImageSubresourceRange,
    pub(crate) handle: vk::ImageView,
}

impl VkImageView {
    pub(crate) fn handle(&self) -> vk::ImageView {
        self.handle
    }
}

impl RhiImageView for VkImageView {
    fn image_id(&self) -> ResourceId {
        self.image_id
    }

    fn subresource_range(&self) -> ImageSubresourceRange {
        self.range
    }
}
