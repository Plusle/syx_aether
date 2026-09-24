use crate::rhi::error::{RhiError, RhiResult};
use ash::vk;
use vma::{Alloc, Allocation, AllocationCreateInfo, AllocatorCreateFlags, AllocatorCreateInfo};

pub(crate) struct Allocator {
    allocator: vma::Allocator,
}

impl Allocator {
    pub(crate) fn new(
        instance: &ash::Instance,
        device: &ash::Device,
        physical_device: vk::PhysicalDevice,
        buffer_device_address: bool,
    ) -> RhiResult<Self> {
        let mut create_info = AllocatorCreateInfo::new(instance, device, physical_device);
        let physical_device_api_version =
            unsafe { instance.get_physical_device_properties(physical_device) }.api_version;
        create_info.vulkan_api_version = physical_device_api_version.min(vk::API_VERSION_1_3);
        if buffer_device_address {
            create_info.flags |= AllocatorCreateFlags::BUFFER_DEVICE_ADDRESS;
        }

        let allocator = unsafe { vma::Allocator::new(create_info) }.map_err(|error| {
            RhiError::BackendError(format!("failed to create Vulkan memory allocator: {error}"))
        })?;

        Ok(Self { allocator })
    }

    pub(crate) fn create_buffer(
        &self,
        buffer_info: &vk::BufferCreateInfo<'_>,
        allocation_info: &AllocationCreateInfo,
    ) -> RhiResult<(vk::Buffer, Allocation)> {
        unsafe { self.allocator.create_buffer(buffer_info, allocation_info) }.map_err(|error| {
            RhiError::BackendError(format!("failed to create Vulkan buffer: {error}"))
        })
    }

    pub(crate) fn destroy_buffer(&self, buffer: vk::Buffer, allocation: &mut Allocation) {
        unsafe { self.allocator.destroy_buffer(buffer, allocation) };
    }
}
