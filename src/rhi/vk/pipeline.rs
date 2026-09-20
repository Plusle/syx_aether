use crate::rhi::pipeline::RhiPipeline;
use ash::vk;

#[derive(Debug)]
pub(crate) struct VkPipeline {
    pub(crate) handle: vk::Pipeline,
    pub(crate) layout: vk::PipelineLayout,
    pub(crate) bind_point: vk::PipelineBindPoint,
}

impl RhiPipeline for VkPipeline {}
