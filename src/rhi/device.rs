#![allow(dead_code)]

use super::backend::RhiDevice;
use super::descriptor::{DescriptorSetLayoutDesc, DescriptorSetLayoutId, PipelineLayoutId};
use super::error::ValidationMessage;
use super::lifetime::{LifetimeTracker, RetireQueue};
use super::memory::FrameResources;
use super::pipeline::{PipelineCache, VertexLayout, VertexLayoutId};
use super::queue::{Queue, QueueInner};
use super::shader::PushConstantRange;
use super::sync::{DeviceId, QueueId};
use super::tracking::ResourceTracker;
use super::types::BackendType;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, RwLock, Weak};

#[derive(Clone, Debug)]
pub struct DeviceDesc {
    pub app_name: String,
    pub backend: BackendType,
    pub validation: bool,
    pub features: Features,
    pub policy: DevicePolicy,
}

#[derive(Clone, Debug)]
pub struct Features {
    pub dynamic_rendering: bool,
    pub synchronization2: bool,
    pub timeline_synchronization: bool,
    pub extended_dynamic_state: bool,
    pub buffer_device_address: bool,
    pub descriptor_indexing: bool,
    pub timestamp_queries: bool,
    /// Test configuration only; no runtime forcing behavior is implemented here.
    pub force_workarounds: bool,
}

#[derive(Clone, Debug)]
pub struct DevicePolicy {
    pub preferred_device: Option<String>,
    pub allow: Vec<DeviceMatch>,
    pub deny: Vec<DeviceMatch>,
}

#[derive(Clone, Debug)]
pub struct DeviceMatch {
    pub vendor_id: Option<u32>,
    pub device_id: Option<u32>,
    pub driver: Option<String>,
}

#[derive(Clone, Debug)]
pub struct DeviceSelection {
    pub backend: BackendType,
    pub device_name: String,
    pub api_version: [u32; 3],
    pub vendor_id: u32,
    pub device_id: u32,
    pub driver: String,
    pub enabled_layers: Vec<String>,
    pub skipped: Vec<SkippedDevice>,
}

#[derive(Clone, Debug)]
pub struct SkippedDevice {
    pub name: String,
    pub vendor_id: u32,
    pub device_id: u32,
    pub reason: String,
}

#[derive(Clone, Debug)]
pub struct Limits {
    pub max_image_dimension_2d: u32,
    pub max_image_dimension_3d: u32,
    pub max_image_array_layers: u32,
    pub max_buffer_size: u64,
    pub max_uniform_buffer_range: u64,
    pub max_storage_buffer_range: u64,
    pub min_uniform_buffer_offset_alignment: u64,
    pub min_storage_buffer_offset_alignment: u64,
    pub max_push_constant_size: u32,
    pub max_bound_descriptor_sets: u32,
    pub max_sampled_images_per_stage: u32,
    pub max_storage_images_per_stage: u32,
    pub max_samplers_per_stage: u32,
    pub max_uniform_buffers_per_stage: u32,
    pub max_storage_buffers_per_stage: u32,
    pub max_color_attachments: u32,
    pub max_vertex_attributes: u32,
    pub max_compute_workgroup_count: [u32; 3],
    pub max_compute_workgroup_size: [u32; 3],
    pub max_compute_workgroup_invocations: u32,
    pub color_sample_counts: Vec<u32>,
    pub depth_sample_counts: Vec<u32>,
    pub timestamp_period_nanoseconds: f64,
    pub max_sampler_anisotropy: f32,
}

#[derive(Clone, Debug)]
pub struct WorkaroundFlag {
    pub name: String,
    pub reason: String,
}

#[derive(Clone, Debug)]
pub struct Capabilities {
    pub limits: Limits,
    pub features: Features,
    pub workarounds: Vec<WorkaroundFlag>,
    pub pipeline_cache_uuid: [u8; 16],
}

#[derive(Clone, Copy, Debug)]
pub struct GcStats {
    pub destroyed: usize,
    pub pending: usize,
    pub reserved: usize,
    pub aliased: usize,
}

/// Unique root. Caches/retire queue are deliberately outside DeviceInner to avoid cycles.
pub struct Device {
    pub(crate) inner: Arc<DeviceInner>,
    pub(crate) queues: HashMap<QueueId, Queue>,
    pub(crate) retire_queue: Arc<RetireQueue>,
    pub(crate) tracker: Arc<ResourceTracker>,
    pub(crate) lifetime: Arc<LifetimeTracker>,
    pub(crate) pipelines: PipelineCache,
    pub(crate) layouts: Mutex<LayoutRegistry>,
    pub(crate) frames: Arc<FrameResources>,
}

pub(crate) struct DeviceInner {
    pub(crate) id: DeviceId,
    pub(crate) backend: Arc<dyn RhiDevice>,
    pub(crate) selection: DeviceSelection,
    pub(crate) capabilities: Capabilities,
    /// Weak registry avoids DeviceInner -> QueueInner -> DeviceInner ownership cycles.
    pub(crate) queues: RwLock<HashMap<QueueId, Weak<QueueInner>>>,
    pub(crate) shutting_down: AtomicBool,
    pub(crate) validation: Mutex<VecDeque<ValidationMessage>>,
}

pub(crate) struct LayoutRegistry {
    pub(crate) sets: HashMap<DescriptorSetLayoutId, DescriptorSetLayoutDesc>,
    pub(crate) pipelines: HashMap<PipelineLayoutId, PipelineLayoutKey>,
    pub(crate) vertices: HashMap<VertexLayoutId, VertexLayout>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PipelineLayoutKey {
    pub(crate) sets: Vec<DescriptorSetLayoutId>,
    pub(crate) push_constants: Vec<PushConstantRange>,
}
