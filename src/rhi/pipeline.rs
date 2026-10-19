#![allow(dead_code)]

use super::backend::RhiPipeline;
use super::descriptor::{PipelineLayout, PipelineLayoutId};
use super::lifetime::SharedObject;
use super::shader::{ShaderId, ShaderModule};
use super::types::*;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PipelineKind {
    Graphics,
    Compute,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct VertexLayoutId(pub(crate) u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VertexInputRate {
    Vertex,
    Instance,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct VertexBufferLayout {
    pub binding: u32,
    pub stride: u32,
    pub rate: VertexInputRate,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct VertexAttribute {
    pub location: u32,
    pub binding: u32,
    pub offset: u32,
    pub format: ImageFormat,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct VertexLayout {
    pub bindings: Vec<VertexBufferLayout>,
    pub attributes: Vec<VertexAttribute>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BlendFactor {
    Zero,
    One,
    SrcColor,
    OneMinusSrcColor,
    DstColor,
    OneMinusDstColor,
    SrcAlpha,
    OneMinusSrcAlpha,
    DstAlpha,
    OneMinusDstAlpha,
    ConstantColor,
    OneMinusConstantColor,
    ConstantAlpha,
    OneMinusConstantAlpha,
    SrcAlphaSaturate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BlendOp {
    Add,
    Subtract,
    ReverseSubtract,
    Min,
    Max,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ColorWriteMask {
    pub red: bool,
    pub green: bool,
    pub blue: bool,
    pub alpha: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BlendAttachmentState {
    pub enabled: bool,
    pub src_color: BlendFactor,
    pub dst_color: BlendFactor,
    pub color_op: BlendOp,
    pub src_alpha: BlendFactor,
    pub dst_alpha: BlendFactor,
    pub alpha_op: BlendOp,
    pub write_mask: ColorWriteMask,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MultisampleState {
    pub sample_count: u32,
    pub sample_mask: Vec<u32>,
    pub sample_shading: Option<FloatBits>,
    pub alpha_to_coverage: bool,
    pub alpha_to_one: bool,
}

/// Only static state belongs here; viewport/scissor and dynamic raster/depth state do not.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct GraphicsState {
    pub topology_class: PrimitiveTopologyClass,
    pub polygon_mode: PolygonMode,
    pub depth_clamp: bool,
    pub rasterizer_discard: bool,
    pub depth_bias_enable: bool,
    pub depth_bounds_test: bool,
    pub stencil_test: bool,
    pub blend: Vec<BlendAttachmentState>,
    pub multisample: MultisampleState,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AttachmentKey {
    pub colors: Vec<ImageFormat>,
    pub depth: Option<ImageFormat>,
    pub stencil: Option<ImageFormat>,
    pub sample_count: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SpecializationValue {
    pub id: u32,
    pub bytes: Vec<u8>,
}

pub struct ShaderEntry<'a> {
    pub module: &'a ShaderModule,
    pub entry_point: &'a str,
}

pub struct GraphicsPipelineDesc<'a> {
    pub name: &'a str,
    pub shaders: &'a [ShaderEntry<'a>],
    pub layout: &'a PipelineLayout,
    pub vertex_layout: &'a VertexLayout,
    pub state: &'a GraphicsState,
    pub attachments: &'a AttachmentKey,
    pub spec_constants: &'a [SpecializationValue],
}

pub struct ComputePipelineDesc<'a> {
    pub name: &'a str,
    pub shader: ShaderEntry<'a>,
    pub layout: &'a PipelineLayout,
    pub spec_constants: &'a [SpecializationValue],
}

pub enum PipelineDesc<'a> {
    Graphics(GraphicsPipelineDesc<'a>),
    Compute(ComputePipelineDesc<'a>),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ShaderEntryKey {
    pub shader: ShaderId,
    pub entry_point: String,
}

/// Full fields, rather than native object identities or hash-only equality.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PipelineKey {
    Graphics {
        shaders: Vec<ShaderEntryKey>,
        vertex_layout: VertexLayoutId,
        layout: PipelineLayoutId,
        attachments: AttachmentKey,
        state: GraphicsState,
        spec_constants: Vec<SpecializationValue>,
    },
    Compute {
        shader: ShaderEntryKey,
        layout: PipelineLayoutId,
        spec_constants: Vec<SpecializationValue>,
    },
}

pub struct Pipeline {
    pub(crate) inner: Arc<SharedObject<dyn RhiPipeline>>,
    pub(crate) kind: PipelineKind,
    pub(crate) layout: PipelineLayoutId,
}

#[derive(Clone, Debug)]
pub struct PipelineCacheDesc {
    pub capacity: usize,
    pub application_version: String,
    pub build_hash: String,
}

#[derive(Clone, Debug)]
pub struct PipelineCacheHeader {
    pub magic: [u8; 8],
    pub application_version: String,
    pub build_hash: String,
    pub backend: BackendType,
    pub vendor_id: u32,
    pub device_id: u32,
    pub driver: String,
    pub cache_uuid: [u8; 16],
    pub shader_abi_version: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub entries: usize,
}

pub struct PipelineCache {
    pub(crate) desc: PipelineCacheDesc,
    pub(crate) state: Mutex<PipelineCacheState>,
}

pub(crate) struct PipelineCacheState {
    pub(crate) entries: HashMap<PipelineKey, Pipeline>,
    pub(crate) lru: VecDeque<PipelineKey>,
    pub(crate) stats: CacheStats,
    pub(crate) driver_data: Vec<u8>,
}
