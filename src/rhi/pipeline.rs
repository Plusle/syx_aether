use crate::rhi::{
    backend::backend,
    error::RhiResult,
    lifetime::{DiscardManager, DiscardedObject, ObjectId, RhiObject},
};
use std::sync::Arc;

/// Backend implementation of a graphics or compute pipeline.
pub(crate) trait RhiPipeline: RhiObject {}

/// Owning, backend-neutral pipeline handle.
#[derive(Debug)]
pub struct Pipeline {
    inner: Option<Arc<dyn RhiPipeline>>,
}

impl Pipeline {
    pub fn new() -> RhiResult<Self> {
        backend()?.create_pipeline()
    }

    pub(crate) fn from_inner(inner: Arc<dyn RhiPipeline>) -> Self {
        Self { inner: Some(inner) }
    }

    pub(crate) fn inner(&self) -> &dyn RhiPipeline {
        self.inner
            .as_deref()
            .expect("live pipeline owns its backend object")
    }
}

impl RhiObject for Pipeline {
    fn id(&self) -> ObjectId {
        self.inner().id()
    }
}

impl RhiPipeline for Pipeline {}

impl Drop for Pipeline {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.take() {
            DiscardManager::global().retire(DiscardedObject::Pipeline(inner));
        }
    }
}
