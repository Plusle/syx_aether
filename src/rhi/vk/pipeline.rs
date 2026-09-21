use crate::rhi::{
    lifetime::{ObjectId, RhiObject},
    pipeline::RhiPipeline,
};
use ash::vk;

#[derive(Debug)]
pub(crate) struct VkPipeline {
    pub(crate) id: ObjectId,
    pub(crate) handle: vk::Pipeline,
    pub(crate) layout: vk::PipelineLayout,
    pub(crate) bind_point: vk::PipelineBindPoint,
}

impl RhiObject for VkPipeline {
    fn id(&self) -> ObjectId {
        self.id
    }
}

impl RhiPipeline for VkPipeline {}
