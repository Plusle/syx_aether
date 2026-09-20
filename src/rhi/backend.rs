#![allow(dead_code)]

use crate::rhi::{
    error::{RhiError, RhiResult},
    queue::{QueueKind, RhiQueue},
    resource::{RhiBuffer, RhiImage, RhiImageView, RhiSampler},
    sync::SyncPoint,
    types::{BufferDesc, ImageDesc, ImageViewDesc, RhiType, SamplerDesc},
};
use std::{sync::Arc, time::Duration};

pub enum BackendType {
    Vulkan,
    Metal,
    D3D12,
}

pub fn bootstrap(t: BackendType) -> Result<(), RhiError> {
    match t {
        BackendType::Vulkan => Ok(()),
        _ => Err(RhiError::UnsupportedBackend),
    }
}

pub fn terminate() {}

pub(crate) trait RhiBackend: RhiType {
    fn queue(&self, kind: QueueKind) -> RhiResult<&dyn RhiQueue>;

    fn create_buffer(&self, desc: BufferDesc) -> RhiResult<Arc<dyn RhiBuffer>>;
    fn create_image(&self, desc: ImageDesc) -> RhiResult<Arc<dyn RhiImage>>;
    fn create_sampler(&self, desc: SamplerDesc) -> RhiResult<Arc<dyn RhiSampler>>;
    fn create_image_view(
        &self,
        img: &Arc<dyn RhiImage>,
        desc: ImageViewDesc,
    ) -> RhiResult<Arc<dyn RhiImageView>>;

    fn destroy_buffer(&self, buffer: Arc<dyn RhiBuffer>);
    fn destroy_image(&self, image: Arc<dyn RhiImage>);
    fn destroy_sampler(&self, sampler: Arc<dyn RhiSampler>);
    fn destroy_image_view(&self, image_view: Arc<dyn RhiImageView>);

    fn wait(&self, point: SyncPoint, timeout: Option<Duration>) -> RhiResult<()>;
}
