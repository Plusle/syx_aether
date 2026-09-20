#![allow(dead_code)]

use crate::rhi::types::RhiType;

pub trait RhiBuffer: RhiType {
    fn name(&self) -> &str;
}

pub trait RhiImage: RhiType {}

pub trait RhiSampler: RhiType {}

pub trait RhiImageView: RhiType {}
