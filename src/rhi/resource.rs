#![allow(dead_code)]

use super::backend::{RhiBuffer, RhiImage, RhiImageView, RhiSampler};
use super::lifetime::{ObjectId, RetireQueue, SharedObject};
use super::types::*;
use std::sync::{Arc, Weak};

#[derive(Clone, Debug)]
pub struct BufferDesc {
    pub name: String,
    pub size: u64,
    pub usage: BufferUsage,
    pub memory: MemoryClass,
}

#[derive(Clone, Debug)]
pub struct ImageDesc {
    pub name: String,
    pub format: ImageFormat,
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub mip_levels: u32,
    pub array_layers: u32,
    pub sample_count: u32,
    pub dimension: ImageDimension,
    pub usage: ImageUsage,
}

#[derive(Clone, Debug)]
pub struct ImageViewDesc {
    pub name: String,
    pub format: ImageFormat,
    pub view_type: ImageViewType,
    pub range: ImageSubresourceRange,
}

#[derive(Clone, Debug)]
pub struct SamplerDesc {
    pub name: String,
    pub min_filter: Filter,
    pub mag_filter: Filter,
    pub mip_filter: Filter,
    pub address_u: AddressMode,
    pub address_v: AddressMode,
    pub address_w: AddressMode,
    pub min_lod: f32,
    pub max_lod: f32,
    pub lod_bias: f32,
    pub max_anisotropy: Option<f32>,
    pub compare: Option<CompareOp>,
    pub border_color: BorderColor,
    pub unnormalized_coordinates: bool,
}

/// Unique public ownership; backend aliases may keep the allocation alive.
pub struct Buffer {
    pub(crate) id: ObjectId,
    pub(crate) desc: BufferDesc,
    pub(crate) inner: Option<Arc<dyn RhiBuffer>>,
    pub(crate) retire_queue: Weak<RetireQueue>,
}

pub struct Image {
    pub(crate) id: ObjectId,
    pub(crate) desc: ImageDesc,
    pub(crate) inner: Option<Arc<dyn RhiImage>>,
    pub(crate) retire_queue: Weak<RetireQueue>,
}

pub struct ImageView {
    pub(crate) id: ObjectId,
    pub(crate) desc: ImageViewDesc,
    pub(crate) inner: Option<Arc<dyn RhiImageView>>,
    pub(crate) image: Arc<dyn RhiImage>,
    pub(crate) retire_queue: Weak<RetireQueue>,
}

/// Shared immutable handle; Clone and final-owner retirement are not implemented yet.
pub struct Sampler {
    pub(crate) inner: Arc<SharedObject<dyn RhiSampler>>,
    pub(crate) desc: SamplerDesc,
}

pub struct BufferSlice {
    pub(crate) buffer: Arc<dyn RhiBuffer>,
    pub(crate) id: ObjectId,
    pub(crate) range: BufferRange,
}
