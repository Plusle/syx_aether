#![allow(dead_code)]
//! Backend-neutral shader reflection: abstract types plus extraction from a
//! linked Slang program layout. The only external dependency here is the
//! compiler's reflection API; nothing is backend-specific.

use std::collections::HashMap;

use shader_slang as slang;

use crate::rhi::error::RhiError;

/// Descriptor kind of a bound resource. Unknown Slang binding types are a
/// compile error, never a silent fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DescriptorType {
    UniformBuffer,
    StorageBuffer,
    SampledImage,
    StorageImage,
    Sampler,
    TexelBuffer,
    StorageTexelBuffer,
}

/// Dimensionality of an image binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewType {
    Undefined,
    Dim1D,
    Dim2D,
    Dim3D,
    Cube,
}

/// Shader stage of one entry point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShaderStage {
    Vertex,
    Hull,
    Domain,
    Geometry,
    Fragment,
    Compute,
}

impl ShaderStage {
    pub fn flag(self) -> ShaderStageFlags {
        match self {
            ShaderStage::Vertex => ShaderStageFlags::VERTEX,
            ShaderStage::Hull => ShaderStageFlags::HULL,
            ShaderStage::Domain => ShaderStageFlags::DOMAIN,
            ShaderStage::Geometry => ShaderStageFlags::GEOMETRY,
            ShaderStage::Fragment => ShaderStageFlags::FRAGMENT,
            ShaderStage::Compute => ShaderStageFlags::COMPUTE,
        }
    }
}

/// Bit set of shader stages, hand-rolled to avoid a bitflags dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShaderStageFlags(pub u32);

impl ShaderStageFlags {
    pub const VERTEX: Self = Self(1 << 0);
    pub const HULL: Self = Self(1 << 1);
    pub const DOMAIN: Self = Self(1 << 2);
    pub const GEOMETRY: Self = Self(1 << 3);
    pub const FRAGMENT: Self = Self(1 << 4);
    pub const COMPUTE: Self = Self(1 << 5);

    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

impl std::ops::BitOr for ShaderStageFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        self.union(rhs)
    }
}

impl std::ops::BitOrAssign for ShaderStageFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

/// Shape category of one UBO field value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueCategory {
    Scalar,
    Vector,
    Matrix,
}

/// Scalar element type of one UBO field value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueType {
    Int,
    UInt,
    Float,
}

/// One member of a uniform buffer block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    pub offset: u32,
    pub size: u32,
    pub padded_size: u32,
    pub element_stride: u32,
    pub count: u32,
    pub category: ValueCategory,
    pub ty: ValueType,
    /// Vector length or matrix row count; 1 for scalars.
    pub dimension: u32,
}

/// One descriptor binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub name: String,
    pub set: u32,
    pub binding: u32,
    pub ty: DescriptorType,
    pub count: u32,
    pub view_type: ViewType,
    pub padded_size: u32,
    pub writable: bool,
    pub readable: bool,
    pub stages: ShaderStageFlags,
}

/// One push constant range. At most one per stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PushConstantRange {
    pub stages: ShaderStageFlags,
    pub offset: u32,
    pub size: u32,
}

/// Full reflection of one compiled shader module.
#[derive(Debug, Default)]
pub struct ShaderReflection {
    pub bindings: Vec<Binding>,
    pub push_constant_ranges: Vec<PushConstantRange>,
    /// UBO block name -> member fields.
    pub fields: HashMap<String, Vec<Field>>,
}

fn align_up(value: usize, alignment: usize) -> usize {
    if alignment == 0 {
        return value;
    }
    value.div_ceil(alignment) * alignment
}

pub(crate) fn stage_from_slang(stage: slang::Stage) -> Option<ShaderStage> {
    match stage {
        slang::Stage::Vertex => Some(ShaderStage::Vertex),
        slang::Stage::Hull => Some(ShaderStage::Hull),
        slang::Stage::Domain => Some(ShaderStage::Domain),
        slang::Stage::Geometry => Some(ShaderStage::Geometry),
        slang::Stage::Fragment => Some(ShaderStage::Fragment),
        slang::Stage::Compute => Some(ShaderStage::Compute),
        _ => None,
    }
}

fn descriptor_type(param: &slang::reflection::VariableLayout) -> Result<DescriptorType, RhiError> {
    use slang::{BindingType, ParameterCategory};
    let ty = param.type_layout();
    let category = param.category();
    let shape = ty.resource_shape();
    let binding_ty = (ty.binding_range_count() > 0).then(|| ty.binding_range_type(0));
    match binding_ty {
        Some(BindingType::ConstantBuffer | BindingType::InlineUniformData) => {
            Ok(DescriptorType::UniformBuffer)
        }
        Some(BindingType::Texture) => Ok(DescriptorType::SampledImage),
        // Upstream bindgen spells this variant "MutableTeture".
        Some(BindingType::MutableTeture) => Ok(DescriptorType::StorageImage),
        Some(BindingType::Sampler) => Ok(DescriptorType::Sampler),
        Some(BindingType::TypedBuffer) => Ok(DescriptorType::TexelBuffer),
        Some(BindingType::MutableTypedBuffer) => Ok(DescriptorType::StorageTexelBuffer),
        Some(BindingType::RawBuffer) => Ok(DescriptorType::StorageBuffer),
        Some(BindingType::MutableRawBuffer) => Ok(DescriptorType::StorageBuffer),
        Some(other) => Err(RhiError::Shader(format!(
            "unsupported binding type {other:?} on '{}'",
            param.name().unwrap_or("")
        ))),
        None => match category {
            ParameterCategory::ConstantBuffer => Ok(DescriptorType::UniformBuffer),
            ParameterCategory::ShaderResource => match shape {
                Some(slang::ResourceShape::SlangTextureBuffer) => Ok(DescriptorType::TexelBuffer),
                _ => Ok(DescriptorType::SampledImage),
            },
            ParameterCategory::UnorderedAccess => match shape {
                Some(slang::ResourceShape::SlangTextureBuffer) => {
                    Ok(DescriptorType::StorageTexelBuffer)
                }
                _ => Ok(DescriptorType::StorageImage),
            },
            ParameterCategory::SamplerState => Ok(DescriptorType::Sampler),
            other => Err(RhiError::Shader(format!(
                "unsupported parameter category {other:?} on '{}'",
                param.name().unwrap_or("")
            ))),
        },
    }
}

fn view_type(param: &slang::reflection::VariableLayout) -> ViewType {
    match param.type_layout().resource_shape() {
        Some(slang::ResourceShape::SlangTexture1d | slang::ResourceShape::SlangTexture1dArray) => {
            ViewType::Dim1D
        }
        Some(
            slang::ResourceShape::SlangTexture2d
            | slang::ResourceShape::SlangTexture2dArray
            | slang::ResourceShape::SlangTexture2dMultisample,
        ) => ViewType::Dim2D,
        Some(slang::ResourceShape::SlangTexture3d) => ViewType::Dim3D,
        Some(
            slang::ResourceShape::SlangTextureCube | slang::ResourceShape::SlangTextureCubeArray,
        ) => ViewType::Cube,
        _ => ViewType::Undefined,
    }
}

fn extract_field(field: &slang::reflection::VariableLayout) -> Field {
    use slang::ParameterCategory as Cat;
    let ty = field.type_layout();
    let name = field.name().unwrap_or("").to_string();
    let offset = field.offset(Cat::Uniform) as u32;
    let size = ty.size(Cat::Uniform) as u32;
    let alignment = ty.alignment(Cat::Uniform).max(1) as usize;
    let element_stride = ty.element_stride(Cat::Uniform) as u32;
    let count = field
        .ty()
        .map(|t| t.total_array_element_count() as u32)
        .unwrap_or(0);
    let count = if count == 0 { 1 } else { count };
    let scalar = ty.scalar_type();
    let rows = ty.row_count().unwrap_or(0);
    let cols = ty.column_count().unwrap_or(0);
    let (category, dimension) = if rows > 1 {
        (ValueCategory::Matrix, rows)
    } else if cols > 1 {
        (ValueCategory::Vector, cols)
    } else {
        (ValueCategory::Scalar, 1)
    };
    let ty = match scalar {
        Some(slang::ScalarType::Int8)
        | Some(slang::ScalarType::Int16)
        | Some(slang::ScalarType::Int32)
        | Some(slang::ScalarType::Int64) => ValueType::Int,
        Some(slang::ScalarType::Uint8)
        | Some(slang::ScalarType::Uint16)
        | Some(slang::ScalarType::Uint32)
        | Some(slang::ScalarType::Uint64) => ValueType::UInt,
        _ => ValueType::Float,
    };
    Field {
        name,
        offset,
        size,
        padded_size: align_up(size as usize, alignment) as u32,
        element_stride,
        count,
        category,
        ty,
        dimension,
    }
}

/// Extracts reflection from a linked program layout. Parameters with an
/// explicit stage keep it; stage-less global parameters take `stages`
/// (the union of the program's entry point stages).
pub(crate) fn extract(
    layout: &slang::reflection::Shader,
    stages: ShaderStageFlags,
) -> Result<ShaderReflection, RhiError> {
    use slang::ParameterCategory as Cat;
    let mut reflection = ShaderReflection::default();
    for param in layout.parameters() {
        let category = param.category();
        let stages = stage_from_slang(param.stage())
            .map(ShaderStage::flag)
            .unwrap_or(stages);
        match category {
            Cat::ConstantBuffer
            | Cat::ShaderResource
            | Cat::UnorderedAccess
            | Cat::SamplerState
            | Cat::DescriptorTableSlot => {
                let set = param.binding_space();
                if set >= 2 {
                    // Bindless convention: spaces >= 2 are not surfaced here.
                    continue;
                }
                let binding = param.binding_index();
                let ty = param.type_layout();
                // Container layouts (ConstantBuffer<T>) carry the payload
                // one level down.
                let payload =
                    if ty.field_count() == 0 && ty.kind() == slang::TypeKind::ConstantBuffer {
                        ty.element_type_layout()
                    } else {
                        ty
                    };
                let descriptor_ty = descriptor_type(param)?;
                let access = payload.resource_access();
                let (readable, writable) = match access {
                    Some(slang::ResourceAccess::Read) => (true, false),
                    Some(slang::ResourceAccess::ReadWrite) | Some(slang::ResourceAccess::Write) => {
                        (true, true)
                    }
                    Some(slang::ResourceAccess::None) => (false, false),
                    _ => {
                        tracing::warn!(
                            "unknown resource access on '{}'; assuming read-only",
                            param.name().unwrap_or("")
                        );
                        (true, false)
                    }
                };
                let count = param
                    .ty()
                    .map(|t| t.total_array_element_count() as u32)
                    .unwrap_or(0);
                let count = if count == 0 { 1 } else { count };
                let padded_size = if descriptor_ty == DescriptorType::UniformBuffer {
                    payload.size(Cat::Uniform) as u32
                } else {
                    0
                };
                reflection.bindings.push(Binding {
                    name: param.name().unwrap_or("").to_string(),
                    set,
                    binding,
                    ty: descriptor_ty,
                    count,
                    view_type: view_type(param),
                    padded_size,
                    writable,
                    readable,
                    stages,
                });
                if descriptor_ty == DescriptorType::UniformBuffer {
                    let fields = payload.fields().map(extract_field).collect::<Vec<_>>();
                    if let Some(name) = param.name() {
                        reflection.fields.insert(name.to_string(), fields);
                    }
                }
            }
            Cat::PushConstantBuffer => {
                debug_assert!(
                    reflection
                        .push_constant_ranges
                        .iter()
                        .all(|r| r.stages != stages),
                    "more than one push constant range per stage"
                );
                if reflection
                    .push_constant_ranges
                    .iter()
                    .any(|r| r.stages == stages)
                {
                    return Err(RhiError::Shader(format!(
                        "more than one push constant range in stages {stages:?}"
                    )));
                }
                let ty = param.type_layout();
                let size = if ty.kind() == slang::TypeKind::ConstantBuffer {
                    ty.element_type_layout().size(Cat::Uniform)
                } else {
                    ty.size(Cat::PushConstantBuffer)
                };
                reflection.push_constant_ranges.push(PushConstantRange {
                    stages,
                    offset: param.offset(Cat::PushConstantBuffer) as u32,
                    size: size as u32,
                });
            }
            _ => {}
        }
    }
    Ok(reflection)
}

/// Merges `src` into `dst`: bindings combine by (set, binding) with
/// count = max and stages unioned; push constant ranges dedupe by stages;
/// UBO fields dedupe by name within each block. Output is sorted by
/// (set, binding) with duplicate keys collapsed.
// Not yet used outside tests: per-module composition lands with the
// pipeline model.
pub(crate) fn merge(dst: &mut ShaderReflection, src: ShaderReflection) {
    for binding in src.bindings {
        match dst
            .bindings
            .iter_mut()
            .find(|b| b.set == binding.set && b.binding == binding.binding)
        {
            Some(existing) => {
                existing.count = existing.count.max(binding.count);
                existing.stages |= binding.stages;
            }
            None => dst.bindings.push(binding),
        }
    }
    for range in src.push_constant_ranges {
        if !dst.push_constant_ranges.contains(&range) {
            dst.push_constant_ranges.push(range);
        }
    }
    for (block, fields) in src.fields {
        let dst_fields = dst.fields.entry(block).or_default();
        for field in fields {
            if !dst_fields.iter().any(|f| f.name == field.name) {
                dst_fields.push(field);
            }
        }
    }
    dst.bindings.sort_by_key(|b| (b.set, b.binding));
    dst.bindings.dedup_by_key(|b| (b.set, b.binding));
}
