use crate::rhi::descriptor::RhiDescriptorSet;
use ash::vk;

#[derive(Debug)]
pub(crate) struct VkDescriptorSet {
    pub(crate) handle: vk::DescriptorSet,
}

impl RhiDescriptorSet for VkDescriptorSet {}
