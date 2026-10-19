#![allow(dead_code)]

use super::backend::{RhiSurface, RhiSwapchain};
use super::resource::{Image, ImageView};
use super::sync::{DeviceId, FrameIndex};
use super::types::{ImageFormat, ImageUsage};
use std::sync::Arc;

/// Platform code supplies the bridge. No window-library or native surface type is public.
pub struct Surface {
    pub(crate) bridge: Arc<dyn RhiSurface>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PresentMode {
    Fifo,
    Mailbox,
}

#[derive(Clone, Debug)]
pub struct SwapchainDesc {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub format: ImageFormat,
    pub present_mode: PresentMode,
    pub frames_in_flight: u32,
}

#[derive(Clone, Debug)]
pub struct SurfaceCapabilities {
    pub formats: Vec<ImageFormat>,
    pub present_modes: Vec<PresentMode>,
    pub min_image_count: u32,
    pub max_image_count: Option<u32>,
    pub min_extent: [u32; 2],
    pub max_extent: [u32; 2],
    pub current_extent: Option<[u32; 2]>,
    pub supported_usage: ImageUsage,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct SwapchainId {
    pub(crate) device: DeviceId,
    pub(crate) serial: u64,
}

/// Identity includes recreation generation and acquisition serial to reject stale tokens.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PresentationId {
    pub(crate) swapchain: SwapchainId,
    pub(crate) generation: u64,
    pub(crate) acquisition: u64,
    pub(crate) image: u32,
    pub(crate) frame: FrameIndex,
}

/// Backend registry keys only; never encoded native semaphores.
pub struct AcquireToken {
    pub(crate) id: PresentationId,
    pub(crate) slot: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PresentToken {
    pub(crate) id: PresentationId,
    pub(crate) slot: u64,
}

/// Wrappers refer to externally owned swapchain images, not ordinary allocated images.
pub struct SwapchainImage {
    pub(crate) image: Image,
    pub(crate) view: ImageView,
    pub(crate) generation: u64,
}

pub struct AcquiredImage {
    pub image: Arc<SwapchainImage>,
    pub index: u32,
    pub frame: FrameIndex,
    pub acquire: AcquireToken,
    pub suboptimal: bool,
}

pub struct Swapchain {
    pub(crate) id: SwapchainId,
    pub(crate) inner: Box<dyn RhiSwapchain>,
    pub(crate) surface: Surface,
    pub(crate) desc: SwapchainDesc,
    pub(crate) images: Vec<Arc<SwapchainImage>>,
    pub(crate) generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresentOutcome {
    Success,
    Suboptimal,
    OutOfDate,
}

/// Acquiring may fail before an image/token exists; present errors use PresentOutcome.
pub enum AcquireOutcome {
    Acquired(AcquiredImage),
    OutOfDate,
    Timeout,
    SurfaceUnavailable,
}
