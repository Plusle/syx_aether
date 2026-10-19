#![allow(dead_code)]

use super::backend::{
    RhiDescriptorPool, RhiDescriptorSet, RhiDescriptorSetLayout, RhiPipelineLayout,
};
use super::lifetime::{ObjectId, ObjectRef, RetireQueue, SharedObject};
use super::resource::{Buffer, ImageView, Sampler};
use super::shader::{Binding, PushConstantRange};
use super::sync::{Access, FrameIndex, SyncPoint};
use super::types::{BufferRange, ResourceRange};
use std::sync::{Arc, Mutex, Weak};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DescriptorSetLayoutId(pub(crate) u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PipelineLayoutId(pub(crate) u64);

#[derive(Clone, Debug)]
pub struct DescriptorSetLayoutDesc {
    pub name: String,
    pub set: u32,
    pub bindings: Vec<Binding>,
}

pub struct DescriptorSetLayout {
    pub(crate) inner: Arc<SharedObject<dyn RhiDescriptorSetLayout>>,
    pub(crate) id: DescriptorSetLayoutId,
    pub(crate) desc: DescriptorSetLayoutDesc,
}

pub struct PipelineLayoutDesc<'a> {
    pub name: &'a str,
    pub sets: &'a [&'a DescriptorSetLayout],
    pub push_constants: &'a [PushConstantRange],
}

pub struct PipelineLayout {
    pub(crate) inner: Arc<SharedObject<dyn RhiPipelineLayout>>,
    pub(crate) id: PipelineLayoutId,
    pub(crate) sets: Vec<Arc<SharedObject<dyn RhiDescriptorSetLayout>>>,
    pub(crate) push_constants: Vec<PushConstantRange>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DescriptorSetLifetime {
    Frame(FrameIndex),
    Persistent,
}

#[derive(Clone, Debug)]
pub struct DescriptorPoolDesc {
    pub name: String,
    pub max_sets: u32,
    pub capacities: Vec<DescriptorCapacity>,
}

#[derive(Clone, Copy, Debug)]
pub struct DescriptorCapacity {
    pub ty: super::shader::DescriptorType,
    pub count: u32,
}

pub struct DescriptorSetDesc<'a> {
    pub name: &'a str,
    pub layout: &'a DescriptorSetLayout,
    pub lifetime: DescriptorSetLifetime,
}

pub struct DescriptorSet {
    pub(crate) inner: Arc<DescriptorSetStorage>,
}

/// Pool ownership keeps written references alive after a transient handle is dropped.
pub(crate) struct DescriptorSetStorage {
    pub(crate) id: ObjectId,
    pub(crate) backend: Arc<dyn RhiDescriptorSet>,
    pub(crate) layout: Arc<SharedObject<dyn RhiDescriptorSetLayout>>,
    pub(crate) lifetime: DescriptorSetLifetime,
    pub(crate) pool_generation: u64,
    pub(crate) written: Mutex<Vec<WrittenResource>>,
    pub(crate) retire_queue: Weak<RetireQueue>,
}

pub(crate) struct WrittenResource {
    pub(crate) binding: u32,
    pub(crate) element: u32,
    pub(crate) id: ObjectId,
    /// None for lifetime-only objects such as samplers.
    pub(crate) access: Option<Access>,
    pub(crate) range: Option<ResourceRange>,
    pub(crate) object: ObjectRef,
}

pub(crate) struct DescriptorPool {
    pub(crate) backend: Arc<dyn RhiDescriptorPool>,
    pub(crate) desc: DescriptorPoolDesc,
    pub(crate) sets: Vec<Arc<DescriptorSetStorage>>,
    pub(crate) generation: u64,
    pub(crate) completion: Option<SyncPoint>,
}

/// Separate image and sampler writes match reflection; arrays address an element.
pub enum DescriptorWrite<'a> {
    UniformBuffer {
        binding: u32,
        element: u32,
        buffer: &'a Buffer,
        range: BufferRange,
    },
    StorageBuffer {
        binding: u32,
        element: u32,
        buffer: &'a Buffer,
        range: BufferRange,
    },
    SampledImage {
        binding: u32,
        element: u32,
        view: &'a ImageView,
    },
    StorageImage {
        binding: u32,
        element: u32,
        view: &'a ImageView,
    },
    Sampler {
        binding: u32,
        element: u32,
        sampler: &'a Sampler,
    },
    InputAttachment {
        binding: u32,
        element: u32,
        view: &'a ImageView,
    },
}
