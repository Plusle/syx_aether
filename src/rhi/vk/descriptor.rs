use crate::rhi::{
    descriptor::RhiDescriptorSet,
    lifetime::{ObjectId, RhiObject},
};
use ash::vk;

#[derive(Debug)]
pub(crate) struct VkDescriptorSet {
    pub(crate) id: ObjectId,
    pub(crate) handle: vk::DescriptorSet,
}

impl RhiObject for VkDescriptorSet {
    fn id(&self) -> ObjectId {
        self.id
    }
}

impl RhiDescriptorSet for VkDescriptorSet {}
