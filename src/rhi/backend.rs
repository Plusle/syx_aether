#![allow(dead_code)]

use crate::rhi::{
    descriptor::{DescriptorSet, RhiDescriptorSet},
    error::{RhiError, RhiResult},
    lifetime::DiscardManager,
    pipeline::{Pipeline, RhiPipeline},
    queue::{Queue, QueueKind, RhiQueue},
    resource::{Buffer, Image, ImageView, RhiBuffer, RhiImage, RhiImageView, RhiSampler, Sampler},
    sync::SyncPoint,
    types::{BufferDesc, ImageDesc, ImageViewDesc, RhiType, SamplerDesc},
    vk::{
        device::{Device, VulkanDevice},
        instance::Instance,
    },
};
use std::{
    sync::{Arc, OnceLock, RwLock},
    time::Duration,
};

type BackendSlot = RwLock<Option<Arc<Backend>>>;

static BACKEND: OnceLock<BackendSlot> = OnceLock::new();

fn backend_slot() -> &'static BackendSlot {
    BACKEND.get_or_init(|| RwLock::new(None))
}

pub enum BackendType {
    Vulkan,
    Metal,
    D3D12,
}

#[derive(Debug, Clone)]
pub struct Backend {
    inner: Arc<dyn RhiBackend>,
}

impl Backend {
    pub fn global() -> RhiResult<Arc<Self>> {
        backend()
    }

    pub(crate) fn create_buffer(&self, desc: BufferDesc) -> RhiResult<Buffer> {
        Ok(Buffer::from_inner(self.inner.create_buffer(desc)?))
    }

    pub(crate) fn create_image(&self, desc: ImageDesc) -> RhiResult<Image> {
        Ok(Image::from_inner(self.inner.create_image(desc)?))
    }

    pub(crate) fn create_sampler(&self, desc: SamplerDesc) -> RhiResult<Sampler> {
        Ok(Sampler::from_inner(self.inner.create_sampler(desc)?))
    }

    pub(crate) fn create_image_view(
        &self,
        image: &Image,
        desc: ImageViewDesc,
    ) -> RhiResult<ImageView> {
        let image = image.inner_arc();
        let inner = self.inner.create_image_view(&image, desc)?;
        Ok(ImageView::from_inner(inner))
    }

    pub(crate) fn create_pipeline(&self) -> RhiResult<Pipeline> {
        Ok(Pipeline::from_inner(self.inner.create_pipeline()?))
    }

    pub(crate) fn create_descriptor_set(&self) -> RhiResult<DescriptorSet> {
        Ok(DescriptorSet::from_inner(
            self.inner.create_descriptor_set()?,
        ))
    }

    pub fn queue(&self, kind: QueueKind) -> RhiResult<Queue> {
        Ok(Queue::new(self.inner.queue(kind)?))
    }

    pub fn wait(&self, point: SyncPoint, timeout: Option<Duration>) -> RhiResult<()> {
        self.inner.wait(point, timeout)
    }

    pub fn wait_all(&self) -> RhiResult<()> {
        self.inner.wait_all()
    }

    pub(crate) fn inner(&self) -> &dyn RhiBackend {
        self.inner.as_ref()
    }
}

pub fn bootstrap(t: BackendType) -> RhiResult<()> {
    let mut current = backend_slot()
        .write()
        .map_err(|_| RhiError::BackendError("global backend lock poisoned".into()))?;
    if current.is_some() {
        return Err(RhiError::BackendError(
            "an RHI backend is already initialized".into(),
        ));
    }

    let inner: Arc<dyn RhiBackend> = match t {
        BackendType::Vulkan => {
            let instance = Arc::new(Instance::new("aether", cfg!(debug_assertions))?);
            let device = Device::new(instance)?;
            Arc::new(VulkanDevice::new(device)?)
        }
        BackendType::Metal | BackendType::D3D12 => return Err(RhiError::UnsupportedBackend),
    };
    *current = Some(Arc::new(Backend { inner }));
    Ok(())
}

pub(crate) fn backend() -> RhiResult<Arc<Backend>> {
    backend_slot()
        .read()
        .map_err(|_| RhiError::BackendError("global backend lock poisoned".into()))?
        .as_ref()
        .cloned()
        .ok_or_else(|| RhiError::BackendError("the RHI backend is not initialized".into()))
}

pub fn terminate() -> RhiResult<()> {
    let backend = backend_slot()
        .read()
        .map_err(|_| RhiError::BackendError("global backend lock poisoned".into()))?
        .as_ref()
        .cloned();
    if let Some(backend) = backend.as_deref() {
        backend.wait_all()?;
        DiscardManager::global().destroy_all()?;
    }
    let b = backend_slot()
        .write()
        .map_err(|_| RhiError::BackendError("global backend lock poisoned".into()))?
        .take();
    drop(b);
    Ok(())
}

pub(crate) trait RhiBackend: RhiType {
    fn queue(&self, kind: QueueKind) -> RhiResult<Arc<dyn RhiQueue>>;

    fn create_buffer(&self, desc: BufferDesc) -> RhiResult<Arc<dyn RhiBuffer>>;
    fn create_image(&self, desc: ImageDesc) -> RhiResult<Arc<dyn RhiImage>>;
    fn create_sampler(&self, desc: SamplerDesc) -> RhiResult<Arc<dyn RhiSampler>>;
    fn create_image_view(
        &self,
        image: &Arc<dyn RhiImage>,
        desc: ImageViewDesc,
    ) -> RhiResult<Arc<dyn RhiImageView>>;
    fn create_pipeline(&self) -> RhiResult<Arc<dyn RhiPipeline>>;
    fn create_descriptor_set(&self) -> RhiResult<Arc<dyn RhiDescriptorSet>>;

    fn destroy_buffer(&self, buffer: Arc<dyn RhiBuffer>);
    fn destroy_image(&self, image: Arc<dyn RhiImage>);
    fn destroy_sampler(&self, sampler: Arc<dyn RhiSampler>);
    fn destroy_image_view(&self, image_view: Arc<dyn RhiImageView>);
    fn destroy_pipeline(&self, pipeline: Arc<dyn RhiPipeline>);
    fn destroy_descriptor_set(&self, descriptor_set: Arc<dyn RhiDescriptorSet>);

    fn wait(&self, point: SyncPoint, timeout: Option<Duration>) -> RhiResult<()>;
    fn wait_all(&self) -> RhiResult<()>;
}
