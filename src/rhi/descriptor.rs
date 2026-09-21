use crate::rhi::{
    backend::backend,
    error::RhiResult,
    lifetime::{DiscardManager, DiscardedObject, ObjectId, RhiObject},
};
use std::sync::Arc;

/// Backend implementation of a bound descriptor set.
pub(crate) trait RhiDescriptorSet: RhiObject {}

/// Owning, backend-neutral descriptor-set handle.
#[derive(Debug)]
pub struct DescriptorSet {
    inner: Option<Arc<dyn RhiDescriptorSet>>,
}

impl DescriptorSet {
    pub fn new() -> RhiResult<Self> {
        backend()?.create_descriptor_set()
    }

    pub(crate) fn from_inner(inner: Arc<dyn RhiDescriptorSet>) -> Self {
        Self { inner: Some(inner) }
    }

    pub(crate) fn inner(&self) -> &dyn RhiDescriptorSet {
        self.inner
            .as_deref()
            .expect("live descriptor set owns its backend object")
    }
}

impl RhiObject for DescriptorSet {
    fn id(&self) -> ObjectId {
        self.inner().id()
    }
}

impl RhiDescriptorSet for DescriptorSet {}

impl Drop for DescriptorSet {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.take() {
            DiscardManager::global().retire(DiscardedObject::Descriptor(inner));
        }
    }
}
