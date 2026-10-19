#![allow(dead_code)]

use super::backend::RhiShaderModule;
use super::lifetime::SharedObject;
use super::types::{ImageFormat, ImageViewType, ShaderStageFlags};
use std::collections::HashMap;
use std::sync::Arc;

pub const SHADER_ABI_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShaderStage {
    Vertex,
    Fragment,
    Compute,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DescriptorType {
    UniformBuffer,
    StorageBuffer,
    SampledImage,
    StorageImage,
    Sampler,
    InputAttachment,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BindingCount {
    Fixed(u32),
    /// Runtime arrays require descriptor indexing; set numbers do not imply bindless.
    Unbounded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BindingAccess {
    Read,
    Write,
    ReadWrite,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Binding {
    pub name: String,
    pub set: u32,
    pub binding: u32,
    pub ty: DescriptorType,
    pub count: BindingCount,
    pub stages: ShaderStageFlags,
    pub access: BindingAccess,
    pub view_type: Option<ImageViewType>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PushConstantRange {
    pub offset: u32,
    pub size: u32,
    pub stages: ShaderStageFlags,
}

#[derive(Clone, Debug)]
pub struct EntryPointInfo {
    pub name: String,
    pub stage: ShaderStage,
    pub workgroup_size: Option<[u32; 3]>,
}

#[derive(Clone, Debug)]
pub struct VertexInput {
    pub name: String,
    pub location: u32,
    pub format: ImageFormat,
}

#[derive(Clone, Debug)]
pub struct FragmentOutput {
    pub name: String,
    pub location: u32,
    pub format: ImageFormat,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ScalarType {
    Bool,
    I32,
    U32,
    F32,
    F64,
}

#[derive(Clone, Debug)]
pub struct SpecConstant {
    pub name: String,
    pub id: u32,
    pub ty: ScalarType,
    pub default_bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct Field {
    pub name: String,
    pub offset: u32,
    pub size: u32,
    pub alignment: u32,
    pub array_stride: Option<u32>,
    pub matrix_stride: Option<u32>,
    pub fields: Vec<Field>,
}

/// Frozen output of reflection, with no independent hand-written shader declaration.
#[derive(Clone, Debug)]
pub struct ShaderInterface {
    pub entry_points: Vec<EntryPointInfo>,
    pub bindings: Vec<Binding>,
    pub push_constants: Vec<PushConstantRange>,
    pub vertex_inputs: Vec<VertexInput>,
    pub fragment_outputs: Vec<FragmentOutput>,
    pub spec_constants: Vec<SpecConstant>,
    pub ubo_fields: HashMap<String, Vec<Field>>,
}

/// Full artifact identity includes source, entries, defines, target, compiler and ABI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ShaderId(pub [u8; 32]);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShaderTarget {
    SpirV,
    MetalLibrary,
    MetalSource,
}

#[derive(Clone, Debug)]
pub struct ShaderArtifact {
    pub target: ShaderTarget,
    pub bytes: Vec<u8>,
    pub entry_points: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct ShaderModuleDesc {
    pub name: String,
    pub id: ShaderId,
    pub abi_version: u32,
    pub artifacts: Vec<ShaderArtifact>,
    pub interface: Arc<ShaderInterface>,
}

pub struct ShaderModule {
    pub(crate) inner: Arc<SharedObject<dyn RhiShaderModule>>,
    pub(crate) id: ShaderId,
    pub(crate) interface: Arc<ShaderInterface>,
}

#[derive(Clone, Debug)]
pub struct ShaderCompileDesc {
    pub source_name: String,
    pub source: String,
    pub entry_points: Vec<String>,
    pub defines: Vec<(String, String)>,
    pub target: ShaderTarget,
    pub target_environment: String,
    pub optimization: ShaderOptimization,
    pub debug_info: bool,
    pub compiler_version: String,
    pub abi_version: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShaderOptimization {
    None,
    Default,
    Maximum,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ShaderJobId(pub(crate) u64);
