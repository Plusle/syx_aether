#![allow(dead_code)]

use super::resource::{VkBuffer, VkImage, VkImageView, VkSampler};
use super::{allocator::Allocator, instance::Instance};
use crate::rhi::{
    backend::RhiBackend,
    error::{RhiError, RhiResult},
    resource::{RhiBuffer, RhiImage, RhiImageView, RhiSampler},
    sync::SyncPoint,
    types::{BufferDesc, ImageDesc, ImageViewDesc, SamplerDesc},
};
use ash::{vk, vk::Handle};
use std::collections::HashSet;
use std::fmt::{Debug, Formatter};
use std::{ffi::CStr, sync::Arc, time::Duration};
use tracing::{info, warn};

pub struct Device {
    // Held only to keep the instance alive for the device's lifetime.
    _instance: Arc<Instance>,
    pub device: ash::Device,
    pub graphics_queue_family: u32,
    pub buffer_device_address: bool,
    allocator: Option<Allocator>,
}

impl Device {
    pub fn new(instance: Arc<Instance>) -> Result<Self, RhiError> {
        let inst = &instance.instance;
        let physical_devices = unsafe { inst.enumerate_physical_devices()? };

        if physical_devices.is_empty() {
            return Err(RhiError::BackendError("No suitable GPU found".into()));
        }

        let mut selected = None;
        for physical_device in physical_devices {
            let props = unsafe { inst.get_physical_device_properties(physical_device) };
            let device_name = unsafe { CStr::from_ptr(props.device_name.as_ptr()) }
                .to_str()
                .unwrap_or("<invalid>")
                .to_string();

            let queue_families =
                unsafe { inst.get_physical_device_queue_family_properties(physical_device) };
            let graphics_family = match queue_families
                .iter()
                .position(|f| f.queue_flags.contains(vk::QueueFlags::GRAPHICS))
            {
                Some(index) => index as u32,
                None => {
                    warn!("Skipping {}: no graphics queue family", device_name);
                    continue;
                }
            };

            // Timeline semaphores are the synchronization backbone; GPUs
            // without them cannot run this backend.
            let mut features12 = vk::PhysicalDeviceVulkan12Features::default();
            let mut features13 = vk::PhysicalDeviceVulkan13Features::default();
            let mut features2 = vk::PhysicalDeviceFeatures2::default()
                .push_next(&mut features12)
                .push_next(&mut features13);
            unsafe { inst.get_physical_device_features2(physical_device, &mut features2) };
            if features12.timeline_semaphore == vk::FALSE {
                warn!(
                    "Skipping {}: timeline semaphores not supported",
                    device_name
                );
                continue;
            }
            if props.api_version < vk::API_VERSION_1_3 || features13.dynamic_rendering == vk::FALSE
            {
                warn!("Skipping {}: dynamic rendering not supported", device_name);
                continue;
            }

            info!("Selected GPU: {}", device_name);
            selected = Some((
                physical_device,
                graphics_family,
                device_name,
                features12.buffer_device_address == vk::TRUE,
            ));
            break;
        }

        let (physical_device, graphics_family, _device_name, buffer_device_address) =
            selected.ok_or(RhiError::BackendError("No suitable GPU found".into()))?;

        let priorities = [1.0f32];
        let queue_create_infos = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(graphics_family)
            .queue_priorities(&priorities)];
        let mut features12 = vk::PhysicalDeviceVulkan12Features::default()
            .timeline_semaphore(true)
            .buffer_device_address(buffer_device_address);
        let mut features13 = vk::PhysicalDeviceVulkan13Features::default().dynamic_rendering(true);
        let create_info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queue_create_infos)
            .push_next(&mut features12)
            .push_next(&mut features13);
        let device = unsafe { inst.create_device(physical_device, &create_info, None)? };

        let allocator = match Allocator::new(inst, &device, physical_device, buffer_device_address)
        {
            Ok(allocator) => allocator,
            Err(error) => {
                unsafe { device.destroy_device(None) };
                return Err(error);
            }
        };

        Ok(Self {
            _instance: instance,
            device,
            graphics_queue_family: graphics_family,
            buffer_device_address,
            allocator: Some(allocator),
        })
    }

    pub(crate) fn allocator(&self) -> &Allocator {
        self.allocator
            .as_ref()
            .expect("Vulkan allocator must exist while Device is alive")
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        // The allocator owns Vulkan memory blocks and must release them while
        // the logical device is still alive.
        let allocator = self.allocator.take();
        drop(allocator);

        unsafe {
            self.device.destroy_device(None);
            info!("Vulkan logical device destroyed");
        }
    }
}

pub struct VulkanDevice {
    ctx: Arc<Device>,
    timelines: HashSet<vk::Semaphore>,
}

impl VulkanDevice {
    pub fn new(ctx: Device) -> Self {
        Self {
            ctx: Arc::new(ctx),
            timelines: HashSet::new(),
        }
    }
}

impl Debug for VulkanDevice {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VulkanDevice").finish()
    }
}

impl RhiBackend for VulkanDevice {
    fn create_buffer(&self, desc: BufferDesc) -> RhiResult<Arc<dyn RhiBuffer>> {
        Ok(Arc::new(VkBuffer::new(Arc::clone(&self.ctx), desc)?))
    }

    fn create_image(&self, _desc: ImageDesc) -> RhiResult<Arc<dyn RhiImage>> {
        Err(RhiError::Unimplemented)
    }

    fn create_sampler(&self, _desc: SamplerDesc) -> RhiResult<Arc<dyn RhiSampler>> {
        Err(RhiError::Unimplemented)
    }

    fn create_image_view(
        &self,
        _image: &Arc<dyn RhiImage>,
        _desc: ImageViewDesc,
    ) -> RhiResult<Arc<dyn RhiImageView>> {
        Err(RhiError::Unimplemented)
    }

    fn destroy_buffer(&self, payload: Arc<dyn RhiBuffer>) {
        // `as_any` exposes the concrete value behind `dyn RhiBuffer`, and
        // `downcast_ref` checks its type before returning a borrowed VkBuffer.
        let Some(_buffer) = payload.as_any().downcast_ref::<VkBuffer>() else {
            debug_assert!(
                false,
                "destroy_buffer called with non-Vulkan buffer payload"
            );
            return;
        };

        // Use `_buffer` here to destroy its Vulkan handle once VkBuffer owns one.
    }

    fn destroy_image(&self, payload: Arc<dyn RhiImage>) {
        debug_assert!(
            payload.as_any().is::<VkImage>(),
            "destroy_image called with non-Vulkan image payload"
        );
    }
    fn destroy_sampler(&self, payload: Arc<dyn RhiSampler>) {
        debug_assert!(
            payload.as_any().is::<VkSampler>(),
            "destroy_sampler called with non-Vulkan sampler payload"
        );
    }
    fn destroy_image_view(&self, payload: Arc<dyn RhiImageView>) {
        debug_assert!(
            payload.as_any().is::<VkImageView>(),
            "destroy_image_view called with non-Vulkan image-view payload"
        );
    }
    fn wait(&self, point: SyncPoint, timeout: Option<Duration>) -> RhiResult<()> {
        let semaphore = vk::Semaphore::from_raw(point.timeline());
        #[cfg(debug_assertions)]
        if !self.timelines.contains(&semaphore) {
            return Err(RhiError::BackendError(
                "wait references an unknown timeline".to_string(),
            ));
        }

        let timeout_ns = timeout
            .map(|t| t.as_nanos().min(u64::MAX as u128) as u64)
            .unwrap_or(u64::MAX);
        let semaphores = [semaphore];
        let values = [point.value()];
        let wait_info = vk::SemaphoreWaitInfo::default()
            .semaphores(&semaphores)
            .values(&values);
        let result = unsafe { self.ctx.device.wait_semaphores(&wait_info, timeout_ns) };
        match result {
            Ok(()) => Ok(()),
            Err(vk::Result::TIMEOUT) => Err(RhiError::BackendError(
                "wait timed out before the sync point was reached".to_string(),
            )),
            Err(err) => Err(err.into()),
        }
    }
}
