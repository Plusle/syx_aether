#![allow(dead_code)]

use crate::rhi::{
    backend::backend,
    command::{Command, ImageSubresourceLayers},
    error::{RhiError, RhiResult},
    lifetime::{DiscardManager, DiscardedObject, LifetimeReservation, ObjectId, RhiObject},
    queue::{QueueId, QueueKind},
    sync::SyncPoint,
    types::{BufferDesc, ImageDesc, ImageViewDesc, SamplerDesc},
};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::{Arc, Mutex},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ImageAspect {
    Color,
    Depth,
    Stencil,
}

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct ImageAspectFlags: u8 {
        const COLOR = 1 << 0;
        const DEPTH = 1 << 1;
        const STENCIL = 1 << 2;
    }
}

impl From<ImageAspect> for ImageAspectFlags {
    fn from(aspect: ImageAspect) -> Self {
        match aspect {
            ImageAspect::Color => Self::COLOR,
            ImageAspect::Depth => Self::DEPTH,
            ImageAspect::Stencil => Self::STENCIL,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ImageSubresourceInfo {
    pub aspects: ImageAspectFlags,
    pub mip_levels: u32,
    pub array_layers: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ImageSubresourceRange {
    aspect: ImageAspect,
    base_mip: u32,
    mip_count: u32,
    base_layer: u32,
    layer_count: u32,
}

impl ImageSubresourceRange {
    pub fn new(
        aspect: ImageAspect,
        base_mip: u32,
        mip_count: u32,
        base_layer: u32,
        layer_count: u32,
    ) -> RhiResult<Self> {
        let range = Self {
            aspect,
            base_mip,
            mip_count,
            base_layer,
            layer_count,
        };
        range.validate_shape()?;
        Ok(range)
    }

    pub const fn aspect(self) -> ImageAspect {
        self.aspect
    }

    pub const fn base_mip(self) -> u32 {
        self.base_mip
    }

    pub const fn mip_count(self) -> u32 {
        self.mip_count
    }

    pub const fn base_layer(self) -> u32 {
        self.base_layer
    }

    pub const fn layer_count(self) -> u32 {
        self.layer_count
    }

    pub fn validate(self, info: ImageSubresourceInfo) -> RhiResult<()> {
        self.validated(info).map(|_| ())
    }

    fn validated(self, info: ImageSubresourceInfo) -> RhiResult<ValidImageRange> {
        validate_image_info(info)?;
        self.validate_shape()?;
        let mip_end = self.base_mip + self.mip_count;
        let layer_end = self.base_layer + self.layer_count;
        if !info.aspects.contains(self.aspect.into()) {
            return Err(RhiError::InvalidCommand(format!(
                "image does not contain the {:?} aspect",
                self.aspect
            )));
        }
        if mip_end > info.mip_levels {
            return Err(RhiError::InvalidCommand(format!(
                "image mip range [{}, {mip_end}) exceeds {} mip levels",
                self.base_mip, info.mip_levels
            )));
        }
        if layer_end > info.array_layers {
            return Err(RhiError::InvalidCommand(format!(
                "image layer range [{}, {layer_end}) exceeds {} array layers",
                self.base_layer, info.array_layers
            )));
        }
        Ok(ValidImageRange {
            aspect: self.aspect,
            mip_start: self.base_mip,
            mip_end,
            layer_start: self.base_layer,
            layer_end,
        })
    }

    fn validate_shape(self) -> RhiResult<()> {
        if self.mip_count == 0 {
            return Err(RhiError::InvalidCommand(
                "image mip count must not be zero".into(),
            ));
        }
        if self.layer_count == 0 {
            return Err(RhiError::InvalidCommand(
                "image layer count must not be zero".into(),
            ));
        }
        self.base_mip
            .checked_add(self.mip_count)
            .ok_or_else(|| RhiError::InvalidCommand("image mip range overflows u32".into()))?;
        self.base_layer
            .checked_add(self.layer_count)
            .ok_or_else(|| RhiError::InvalidCommand("image layer range overflows u32".into()))?;
        Ok(())
    }
}

fn validate_image_info(info: ImageSubresourceInfo) -> RhiResult<()> {
    if info.aspects.is_empty() || !ImageAspectFlags::all().contains(info.aspects) {
        return Err(RhiError::InvalidCommand(
            "image has an invalid aspect mask".into(),
        ));
    }
    if info.mip_levels == 0 {
        return Err(RhiError::InvalidCommand(
            "image must contain at least one mip level".into(),
        ));
    }
    if info.array_layers == 0 {
        return Err(RhiError::InvalidCommand(
            "image must contain at least one array layer".into(),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ValidImageRange {
    aspect: ImageAspect,
    mip_start: u32,
    mip_end: u32,
    layer_start: u32,
    layer_end: u32,
}

pub(crate) trait RhiBuffer: RhiObject {
    fn name(&self) -> &str;
    fn size(&self) -> u64;
}

pub(crate) trait RhiImage: RhiObject {
    fn subresource_info(&self) -> ImageSubresourceInfo;
}

pub(crate) trait RhiSampler: RhiObject {}

pub(crate) trait RhiImageView: RhiObject {
    fn image(&self) -> &dyn RhiImage;
    fn subresource_range(&self) -> ImageSubresourceRange;

    fn image_id(&self) -> ObjectId {
        self.image().id()
    }

    fn image_info(&self) -> ImageSubresourceInfo {
        self.image().subresource_info()
    }
}

#[derive(Debug)]
pub struct Buffer {
    inner: Option<Arc<dyn RhiBuffer>>,
}

impl Buffer {
    pub fn new(desc: BufferDesc) -> RhiResult<Self> {
        backend()?.create_buffer(desc)
    }

    pub(crate) fn from_inner(inner: Arc<dyn RhiBuffer>) -> Self {
        Self { inner: Some(inner) }
    }

    pub(crate) fn inner(&self) -> &dyn RhiBuffer {
        self.inner
            .as_deref()
            .expect("live buffer owns its backend object")
    }

    pub fn name(&self) -> &str {
        self.inner().name()
    }

    pub fn size(&self) -> u64 {
        self.inner().size()
    }
}

impl RhiObject for Buffer {
    fn id(&self) -> ObjectId {
        self.inner().id()
    }
}

impl RhiBuffer for Buffer {
    fn name(&self) -> &str {
        self.inner().name()
    }

    fn size(&self) -> u64 {
        self.inner().size()
    }
}

impl Drop for Buffer {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.take() {
            DiscardManager::global().retire(DiscardedObject::Buffer(inner));
        }
    }
}

#[derive(Debug)]
pub struct Image {
    inner: Option<Arc<dyn RhiImage>>,
}

impl Image {
    pub fn new(desc: ImageDesc) -> RhiResult<Self> {
        backend()?.create_image(desc)
    }

    pub(crate) fn from_inner(inner: Arc<dyn RhiImage>) -> Self {
        Self { inner: Some(inner) }
    }

    pub(crate) fn inner(&self) -> &dyn RhiImage {
        self.inner
            .as_deref()
            .expect("live image owns its backend object")
    }

    pub(crate) fn inner_arc(&self) -> Arc<dyn RhiImage> {
        Arc::clone(
            self.inner
                .as_ref()
                .expect("live image owns its backend object"),
        )
    }

    pub fn subresource_info(&self) -> ImageSubresourceInfo {
        self.inner().subresource_info()
    }
}

impl RhiObject for Image {
    fn id(&self) -> ObjectId {
        self.inner().id()
    }
}

impl RhiImage for Image {
    fn subresource_info(&self) -> ImageSubresourceInfo {
        self.inner().subresource_info()
    }
}

impl Drop for Image {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.take() {
            DiscardManager::global().retire(DiscardedObject::Image(inner));
        }
    }
}

#[derive(Debug)]
pub struct Sampler {
    inner: Option<Arc<dyn RhiSampler>>,
}

impl Sampler {
    pub fn new(desc: SamplerDesc) -> RhiResult<Self> {
        backend()?.create_sampler(desc)
    }

    pub(crate) fn from_inner(inner: Arc<dyn RhiSampler>) -> Self {
        Self { inner: Some(inner) }
    }

    pub(crate) fn inner(&self) -> &dyn RhiSampler {
        self.inner
            .as_deref()
            .expect("live sampler owns its backend object")
    }
}

impl RhiObject for Sampler {
    fn id(&self) -> ObjectId {
        self.inner().id()
    }
}

impl RhiSampler for Sampler {}

impl Drop for Sampler {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.take() {
            DiscardManager::global().retire(DiscardedObject::Sampler(inner));
        }
    }
}

#[derive(Debug)]
pub struct ImageView {
    inner: Option<Arc<dyn RhiImageView>>,
}

impl ImageView {
    pub fn new(image: &Image, desc: ImageViewDesc) -> RhiResult<Self> {
        backend()?.create_image_view(image, desc)
    }

    pub(crate) fn from_inner(inner: Arc<dyn RhiImageView>) -> Self {
        Self { inner: Some(inner) }
    }

    pub(crate) fn inner(&self) -> &dyn RhiImageView {
        self.inner
            .as_deref()
            .expect("live image view owns its backend object")
    }

    pub fn subresource_range(&self) -> ImageSubresourceRange {
        self.inner().subresource_range()
    }
}

impl RhiObject for ImageView {
    fn id(&self) -> ObjectId {
        self.inner().id()
    }
}

impl RhiImageView for ImageView {
    fn image(&self) -> &dyn RhiImage {
        self.inner().image()
    }

    fn subresource_range(&self) -> ImageSubresourceRange {
        self.inner().subresource_range()
    }
}

impl Drop for ImageView {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.take() {
            DiscardManager::global().retire(DiscardedObject::ImageView(inner));
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ResourceRange {
    Buffer { offset: u64, size: u64 },
    Image(ImageSubresourceRange),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Access {
    VertexRead,
    IndexRead,
    IndirectRead,
    TransferRead,
    TransferWrite,
    ShaderRead,
    ShaderWrite,
    ColorAttachmentReadWrite,
    DepthStencilReadWrite,
}

impl Access {
    fn is_write(self) -> bool {
        matches!(
            self,
            Self::TransferWrite
                | Self::ShaderWrite
                | Self::ColorAttachmentReadWrite
                | Self::DepthStencilReadWrite
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ResourceState {
    pub access: Access,
    pub queue: QueueId,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum ResourceRef<'a> {
    Buffer(&'a dyn RhiBuffer),
    Image(&'a dyn RhiImage),
}

impl ResourceRef<'_> {
    pub(crate) fn id(self) -> ObjectId {
        match self {
            Self::Buffer(resource) => resource.id(),
            Self::Image(resource) => resource.id(),
        }
    }

    fn is_image(self) -> bool {
        !matches!(self, Self::Buffer(_))
    }

    fn image_info(self) -> Option<ImageSubresourceInfo> {
        match self {
            Self::Buffer(_) => None,
            Self::Image(resource) => Some(resource.subresource_info()),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Transition<'a> {
    pub resource: ResourceRef<'a>,
    pub range: ResourceRange,
    pub before: Option<ResourceState>,
    pub after: ResourceState,
    pub kind: TransitionKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransitionKind {
    Memory,
    Release,
    Acquire,
}

#[derive(Debug)]
pub(crate) enum PlannedOp<'a> {
    Barriers(Vec<Transition<'a>>),
    Command(Command<'a>),
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ResourceUse<'a> {
    pub(crate) resource: ResourceRef<'a>,
    range: ValidResourceRange,
    pub(crate) access: Access,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValidResourceRange {
    Buffer { offset: u64, size: u64 },
    Image(ValidImageRange),
}

#[derive(Debug)]
enum AnalyzedOp<'a> {
    Command {
        command: Command<'a>,
        uses: Vec<ResourceUse<'a>>,
    },
    Release {
        resources: Vec<ResourceRef<'a>>,
        target: QueueKind,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BufferRangeState {
    offset: u64,
    size: u64,
    state: Option<ResourceState>,
    released_to: Option<QueueId>,
}

impl BufferRangeState {
    fn range(self) -> ResourceRange {
        ResourceRange::Buffer {
            offset: self.offset,
            size: self.size,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct ImageSubresourceKey {
    aspect: ImageAspect,
    mip: u32,
    layer: u32,
}

impl ImageSubresourceKey {
    fn range(self) -> ResourceRange {
        ResourceRange::Image(ImageSubresourceRange {
            aspect: self.aspect,
            base_mip: self.mip,
            mip_count: 1,
            base_layer: self.layer,
            layer_count: 1,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ImageCellState {
    state: ResourceState,
    released_to: Option<QueueId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ImageStates {
    info: ImageSubresourceInfo,
    cells: BTreeMap<ImageSubresourceKey, ImageCellState>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
enum TrackedStates {
    #[default]
    Uninitialized,
    Buffer(Vec<BufferRangeState>),
    Image(ImageStates),
}

#[derive(Debug, Default)]
struct TrackedResource {
    states: TrackedStates,
}

#[derive(Debug, Default)]
pub(crate) struct ResourceTracker {
    resources: Mutex<HashMap<ObjectId, TrackedResource>>,
}

impl ResourceTracker {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn forget(&self, id: ObjectId) {
        self.resources
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&id);
    }

    pub(crate) fn plan<'a>(
        self: &Arc<Self>,
        queue: QueueId,
        commands: &[Command<'a>],
        lifetime: &LifetimeReservation,
        resolve_queue: impl Fn(QueueKind) -> RhiResult<QueueId>,
    ) -> RhiResult<(Vec<PlannedOp<'a>>, TrackerTransaction)> {
        let analyzed = analyze(commands)?;
        let touched: HashSet<_> = analyzed
            .iter()
            .flat_map(|operation| match operation {
                AnalyzedOp::Command { uses, .. } => uses
                    .iter()
                    .map(|usage| usage.resource.id())
                    .collect::<Vec<_>>(),
                AnalyzedOp::Release { resources, .. } => {
                    resources.iter().map(|resource| resource.id()).collect()
                }
            })
            .collect();
        let resources = self
            .resources
            .lock()
            .map_err(|_| RhiError::ResourceState("resource tracker poisoned".into()))?;

        let mut local: HashMap<_, TrackedStates> = touched
            .iter()
            .map(|id| {
                (
                    *id,
                    resources
                        .get(id)
                        .map(|resource| resource.states.clone())
                        .unwrap_or_default(),
                )
            })
            .collect();
        drop(resources);

        let mut transaction = TrackerTransaction {
            tracker: Arc::clone(self),
            states: HashMap::new(),
            waits: Vec::new(),
        };

        let mut waits = HashSet::new();
        let mut planned = Vec::with_capacity(commands.len() * 2);
        for operation in analyzed {
            let AnalyzedOp::Command { command, uses } = operation else {
                let AnalyzedOp::Release { resources, target } = operation else {
                    unreachable!()
                };
                let target = resolve_queue(target)?;
                if target == queue {
                    return Err(RhiError::ResourceState(
                        "a queue cannot release ownership to itself".into(),
                    ));
                }
                let mut releases = Vec::new();
                for resource in resources {
                    let states = local
                        .get_mut(&resource.id())
                        .expect("release resource must be reserved");
                    let released =
                        release_resource(states, resource, queue, target, &mut releases)?;
                    if !released {
                        return Err(RhiError::ResourceState(format!(
                            "resource {} cannot be released before its first use",
                            resource.id().get()
                        )));
                    }
                }
                planned.push(PlannedOp::Barriers(releases));
                continue;
            };
            let mut transitions = Vec::new();
            for usage in uses {
                let id = usage.resource.id();
                let states = local.entry(id).or_default();
                if plan_resource_use(states, usage, queue, &mut transitions)? {
                    waits.extend(lifetime.last_uses(id));
                }
            }
            if !transitions.is_empty() {
                planned.push(PlannedOp::Barriers(transitions));
            }
            planned.push(PlannedOp::Command(command));
        }

        transaction.states = local;
        transaction.waits = waits.into_iter().collect();
        Ok((planned, transaction))
    }
}

#[derive(Debug)]
pub(crate) struct TrackerTransaction {
    tracker: Arc<ResourceTracker>,
    states: HashMap<ObjectId, TrackedStates>,
    pub(crate) waits: Vec<SyncPoint>,
}

impl TrackerTransaction {
    pub(crate) fn commit(mut self) {
        let mut resources = self
            .tracker
            .resources
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        for (id, states) in self.states.drain() {
            resources.entry(id).or_default().states = states;
        }
        drop(resources);
    }
}

fn release_resource<'a>(
    states: &mut TrackedStates,
    resource: ResourceRef<'a>,
    queue: QueueId,
    target: QueueId,
    releases: &mut Vec<Transition<'a>>,
) -> RhiResult<bool> {
    let mut released = false;
    match states {
        TrackedStates::Uninitialized => {}
        TrackedStates::Buffer(states) => {
            if !matches!(resource, ResourceRef::Buffer(_)) {
                return Err(resource_kind_mismatch(resource.id()));
            }
            for current in states {
                if current.released_to.is_some() {
                    return Err(already_released(resource.id()));
                }
                let Some(before) = current.state else {
                    continue;
                };
                if before.queue != queue {
                    return Err(not_owned_by_queue(resource.id()));
                }
                releases.push(Transition {
                    resource,
                    range: current.range(),
                    before: Some(before),
                    after: ResourceState {
                        access: before.access,
                        queue: target,
                    },
                    kind: TransitionKind::Release,
                });
                current.released_to = Some(target);
                released = true;
            }
        }
        TrackedStates::Image(states) => {
            if !resource.is_image() {
                return Err(resource_kind_mismatch(resource.id()));
            }
            for (key, current) in &mut states.cells {
                if current.released_to.is_some() {
                    return Err(already_released(resource.id()));
                }
                if current.state.queue != queue {
                    return Err(not_owned_by_queue(resource.id()));
                }
                releases.push(Transition {
                    resource,
                    range: key.range(),
                    before: Some(current.state),
                    after: ResourceState {
                        access: current.state.access,
                        queue: target,
                    },
                    kind: TransitionKind::Release,
                });
                current.released_to = Some(target);
                released = true;
            }
        }
    }
    Ok(released)
}

fn plan_resource_use<'a>(
    states: &mut TrackedStates,
    usage: ResourceUse<'a>,
    queue: QueueId,
    transitions: &mut Vec<Transition<'a>>,
) -> RhiResult<bool> {
    match usage.range {
        ValidResourceRange::Buffer { offset, size } => {
            let ResourceRef::Buffer(buffer) = usage.resource else {
                return Err(resource_kind_mismatch(usage.resource.id()));
            };
            if matches!(states, TrackedStates::Uninitialized) {
                *states = TrackedStates::Buffer(vec![BufferRangeState {
                    offset: 0,
                    size: buffer.size(),
                    state: None,
                    released_to: None,
                }]);
            }
            let TrackedStates::Buffer(states) = states else {
                return Err(resource_kind_mismatch(usage.resource.id()));
            };
            plan_buffer_use(states, usage, offset, size, queue, transitions)
        }
        ValidResourceRange::Image(range) => {
            let info = usage
                .resource
                .image_info()
                .ok_or_else(|| resource_kind_mismatch(usage.resource.id()))?;
            if matches!(states, TrackedStates::Uninitialized) {
                *states = TrackedStates::Image(ImageStates {
                    info,
                    cells: BTreeMap::new(),
                });
            }
            let TrackedStates::Image(states) = states else {
                return Err(resource_kind_mismatch(usage.resource.id()));
            };
            if states.info != info {
                return Err(RhiError::ResourceState(format!(
                    "resource {} was referenced with inconsistent image metadata",
                    usage.resource.id().get()
                )));
            }
            plan_image_use(states, usage, range, queue, transitions)
        }
    }
}

fn plan_buffer_use<'a>(
    states: &mut Vec<BufferRangeState>,
    usage: ResourceUse<'a>,
    offset: u64,
    size: u64,
    queue: QueueId,
    transitions: &mut Vec<Transition<'a>>,
) -> RhiResult<bool> {
    let after = ResourceState {
        access: usage.access,
        queue,
    };
    let overlapping: Vec<_> = states
        .iter()
        .filter(|state| buffer_overlaps(state.offset, state.size, offset, size))
        .copied()
        .collect();
    let mut did_acquire = false;
    for current in overlapping {
        if let Some(target) = current.released_to {
            if target != queue {
                return Err(released_to_different_queue(usage.resource.id()));
            }
            let before = current
                .state
                .expect("released buffer ranges always have a state");
            let acquired = ResourceState {
                access: before.access,
                queue,
            };
            transitions.push(Transition {
                resource: usage.resource,
                range: current.range(),
                before: Some(before),
                after: acquired,
                kind: TransitionKind::Acquire,
            });
            replace_buffer_state(states, current.offset, current.size, acquired);
            did_acquire = true;
            if needs_transition(Some(acquired), after, false) {
                let (intersection_offset, intersection_size) =
                    buffer_intersection(current.offset, current.size, offset, size);
                transitions.push(Transition {
                    resource: usage.resource,
                    range: ResourceRange::Buffer {
                        offset: intersection_offset,
                        size: intersection_size,
                    },
                    before: Some(acquired),
                    after,
                    kind: TransitionKind::Memory,
                });
            }
            continue;
        }
        if current.state.is_some_and(|before| before.queue != queue) {
            return Err(crossed_queues_without_release(usage.resource.id()));
        }
        if needs_transition(current.state, after, false) {
            let (intersection_offset, intersection_size) =
                buffer_intersection(current.offset, current.size, offset, size);
            transitions.push(Transition {
                resource: usage.resource,
                range: ResourceRange::Buffer {
                    offset: intersection_offset,
                    size: intersection_size,
                },
                before: current.state,
                after,
                kind: TransitionKind::Memory,
            });
        }
    }
    replace_buffer_state(states, offset, size, after);
    Ok(did_acquire)
}

fn plan_image_use<'a>(
    states: &mut ImageStates,
    usage: ResourceUse<'a>,
    range: ValidImageRange,
    queue: QueueId,
    transitions: &mut Vec<Transition<'a>>,
) -> RhiResult<bool> {
    let after = ResourceState {
        access: usage.access,
        queue,
    };
    let mut did_acquire = false;
    for mip in range.mip_start..range.mip_end {
        for layer in range.layer_start..range.layer_end {
            let key = ImageSubresourceKey {
                aspect: range.aspect,
                mip,
                layer,
            };
            let current = states.cells.get(&key).copied();
            if let Some(current) = current {
                if let Some(target) = current.released_to {
                    if target != queue {
                        return Err(released_to_different_queue(usage.resource.id()));
                    }
                    let acquired = ResourceState {
                        access: current.state.access,
                        queue,
                    };
                    transitions.push(Transition {
                        resource: usage.resource,
                        range: key.range(),
                        before: Some(current.state),
                        after: acquired,
                        kind: TransitionKind::Acquire,
                    });
                    did_acquire = true;
                    if needs_transition(Some(acquired), after, true) {
                        transitions.push(Transition {
                            resource: usage.resource,
                            range: key.range(),
                            before: Some(acquired),
                            after,
                            kind: TransitionKind::Memory,
                        });
                    }
                    states.cells.insert(
                        key,
                        ImageCellState {
                            state: after,
                            released_to: None,
                        },
                    );
                    continue;
                }
                if current.state.queue != queue {
                    return Err(crossed_queues_without_release(usage.resource.id()));
                }
            }
            let before = current.map(|current| current.state);
            if needs_transition(before, after, true) {
                transitions.push(Transition {
                    resource: usage.resource,
                    range: key.range(),
                    before,
                    after,
                    kind: TransitionKind::Memory,
                });
            }
            states.cells.insert(
                key,
                ImageCellState {
                    state: after,
                    released_to: None,
                },
            );
        }
    }
    Ok(did_acquire)
}

fn resource_kind_mismatch(id: ObjectId) -> RhiError {
    RhiError::ResourceState(format!(
        "resource {} was used as both a buffer and an image",
        id.get()
    ))
}

fn already_released(id: ObjectId) -> RhiError {
    RhiError::ResourceState(format!("resource {} has already been released", id.get()))
}

fn not_owned_by_queue(id: ObjectId) -> RhiError {
    RhiError::ResourceState(format!(
        "queue does not own resource {} and cannot release it",
        id.get()
    ))
}

fn released_to_different_queue(id: ObjectId) -> RhiError {
    RhiError::ResourceState(format!(
        "resource {} was released to a different queue",
        id.get()
    ))
}

fn crossed_queues_without_release(id: ObjectId) -> RhiError {
    RhiError::ResourceState(format!(
        "resource {} crossed queues without an ownership release",
        id.get()
    ))
}

fn analyze<'a>(commands: &[Command<'a>]) -> RhiResult<Vec<AnalyzedOp<'a>>> {
    let mut rendering = false;
    let mut label_depth = 0usize;
    let mut result = Vec::with_capacity(commands.len());

    for command in commands.iter().copied() {
        let release = match command {
            Command::ReleaseBuffers { buffers, target } => Some((
                buffers
                    .iter()
                    .map(|buffer| ResourceRef::Buffer(*buffer))
                    .collect::<Vec<_>>(),
                target,
            )),
            Command::ReleaseImages { images, target } => Some((
                images
                    .iter()
                    .map(|image| ResourceRef::Image(*image))
                    .collect::<Vec<_>>(),
                target,
            )),
            _ => None,
        };
        if let Some((resources, target)) = release {
            if rendering {
                return Err(RhiError::InvalidCommand(
                    "queue ownership cannot be released inside a rendering pass".into(),
                ));
            }
            if resources.is_empty() {
                return Err(RhiError::InvalidCommand(
                    "a release command must contain at least one resource".into(),
                ));
            }
            let mut ids = HashSet::new();
            if resources.iter().any(|resource| !ids.insert(resource.id())) {
                return Err(RhiError::InvalidCommand(
                    "a release command contains the same resource more than once".into(),
                ));
            }
            result.push(AnalyzedOp::Release { resources, target });
            continue;
        }

        let uses = match command {
            Command::BeginRendering(desc) => {
                if rendering {
                    return Err(RhiError::InvalidCommand(
                        "rendering passes cannot be nested".into(),
                    ));
                }
                rendering = true;
                let mut uses = Vec::new();
                for attachment in desc.color_attachments {
                    uses.push(image_view_use(
                        attachment.view,
                        Access::ColorAttachmentReadWrite,
                    )?);
                    if let Some(resolve) = attachment.resolve {
                        uses.push(image_view_use(resolve, Access::ColorAttachmentReadWrite)?);
                    }
                }
                if let Some(attachment) = desc.depth_stencil {
                    uses.push(image_view_use(
                        attachment.view,
                        Access::DepthStencilReadWrite,
                    )?);
                    if let Some(resolve) = attachment.resolve {
                        uses.push(image_view_use(resolve, Access::DepthStencilReadWrite)?);
                    }
                }
                uses
            }
            Command::EndRendering => {
                if !rendering {
                    return Err(RhiError::InvalidCommand(
                        "EndRendering has no matching BeginRendering".into(),
                    ));
                }
                rendering = false;
                Vec::new()
            }
            Command::Draw { .. }
            | Command::DrawIndexed { .. }
            | Command::DrawIndirect { .. }
            | Command::DrawIndexedIndirect { .. }
            if !rendering =>
                {
                    return Err(RhiError::InvalidCommand(
                        "draw commands must be inside a rendering pass".into(),
                    ));
                }
            Command::Dispatch(_) | Command::DispatchIndirect { .. } if rendering => {
                return Err(RhiError::InvalidCommand(
                    "dispatch commands must be outside a rendering pass".into(),
                ));
            }
            Command::DrawIndirect { buffer, offset, .. }
            | Command::DrawIndexedIndirect { buffer, offset, .. }
            | Command::DispatchIndirect { buffer, offset } => vec![buffer_use(
                buffer,
                offset,
                buffer.size().saturating_sub(offset),
                Access::IndirectRead,
            )?],
            Command::CopyBuffer {
                src,
                src_offset,
                dst,
                dst_offset,
                size,
            } => vec![
                buffer_use(src, src_offset, size, Access::TransferRead)?,
                buffer_use(dst, dst_offset, size, Access::TransferWrite)?,
            ],
            Command::CopyBufferToImage { src, dst, region } => vec![
                buffer_use(
                    src,
                    region.buffer_offset,
                    src.size().saturating_sub(region.buffer_offset),
                    Access::TransferRead,
                )?,
                image_use(
                    ResourceRef::Image(dst),
                    region.image_subresource,
                    Access::TransferWrite,
                )?,
            ],
            Command::CopyImageToBuffer { src, dst, region } => vec![
                image_use(
                    ResourceRef::Image(src),
                    region.image_subresource,
                    Access::TransferRead,
                )?,
                buffer_use(
                    dst,
                    region.buffer_offset,
                    dst.size().saturating_sub(region.buffer_offset),
                    Access::TransferWrite,
                )?,
            ],
            Command::CopyImage {
                src,
                src_region,
                dst,
                dst_region,
                ..
            } => vec![
                image_use(
                    ResourceRef::Image(src),
                    src_region.subresource,
                    Access::TransferRead,
                )?,
                image_use(
                    ResourceRef::Image(dst),
                    dst_region.subresource,
                    Access::TransferWrite,
                )?,
            ],
            Command::BindVertexBuffers { buffers, .. } => buffers
                .iter()
                .map(|(buffer, offset)| {
                    buffer_use(
                        *buffer,
                        *offset,
                        buffer.size().saturating_sub(*offset),
                        Access::VertexRead,
                    )
                })
                .collect::<RhiResult<Vec<_>>>()?,
            Command::BindIndexBuffer { buffer, offset, .. } => vec![buffer_use(
                buffer,
                offset,
                buffer.size().saturating_sub(offset),
                Access::IndexRead,
            )?],
            Command::BindDescriptorSets { sets, .. } if !sets.is_empty() => {
                return Err(RhiError::InvalidCommand(
                    "descriptor resource introspection is not implemented yet".into(),
                ));
            }
            Command::BeginDebugLabel(_) => {
                label_depth += 1;
                Vec::new()
            }
            Command::EndDebugLabel => {
                if label_depth == 0 {
                    return Err(RhiError::InvalidCommand(
                        "EndDebugLabel has no matching BeginDebugLabel".into(),
                    ));
                }
                label_depth -= 1;
                Vec::new()
            }
            Command::ReleaseBuffers { .. } | Command::ReleaseImages { .. } => unreachable!(),
            _ => Vec::new(),
        };
        result.push(AnalyzedOp::Command { command, uses });
    }

    if rendering {
        return Err(RhiError::InvalidCommand(
            "rendering pass was not ended".into(),
        ));
    }
    if label_depth != 0 {
        return Err(RhiError::InvalidCommand(
            "debug label region was not ended".into(),
        ));
    }
    Ok(result)
}

fn buffer_use<'a>(
    buffer: &'a dyn RhiBuffer,
    offset: u64,
    size: u64,
    access: Access,
) -> RhiResult<ResourceUse<'a>> {
    let end = offset
        .checked_add(size)
        .ok_or_else(|| RhiError::InvalidCommand("buffer range overflows u64".into()))?;
    if size == 0 || end > buffer.size() {
        return Err(RhiError::InvalidCommand(format!(
            "buffer range [{offset}, {end}) exceeds buffer size {}",
            buffer.size()
        )));
    }
    Ok(ResourceUse {
        resource: ResourceRef::Buffer(buffer),
        range: ValidResourceRange::Buffer { offset, size },
        access,
    })
}

fn image_use(
    resource: ResourceRef<'_>,
    layers: ImageSubresourceLayers,
    access: Access,
) -> RhiResult<ResourceUse<'_>> {
    let info = resource
        .image_info()
        .ok_or_else(|| resource_kind_mismatch(resource.id()))?;
    let range = ImageSubresourceRange {
        aspect: layers.aspect,
        base_mip: layers.mip,
        mip_count: 1,
        base_layer: layers.base_layer,
        layer_count: layers.layer_count,
    }
        .validated(info)?;
    Ok(ResourceUse {
        resource,
        range: ValidResourceRange::Image(range),
        access,
    })
}

fn image_view_use(view: &dyn RhiImageView, access: Access) -> RhiResult<ResourceUse<'_>> {
    let range = view.subresource_range().validated(view.image_info())?;
    Ok(ResourceUse {
        resource: ResourceRef::Image(view.image()),
        range: ValidResourceRange::Image(range),
        access,
    })
}

fn needs_transition(before: Option<ResourceState>, after: ResourceState, image: bool) -> bool {
    let Some(before) = before else {
        return image;
    };
    before.queue != after.queue
        || before.access != after.access
        || before.access.is_write()
        || after.access.is_write()
}

fn buffer_overlaps(a: u64, a_size: u64, b: u64, b_size: u64) -> bool {
    a < b + b_size && b < a + a_size
}

fn buffer_intersection(a: u64, a_size: u64, b: u64, b_size: u64) -> (u64, u64) {
    let start = a.max(b);
    let end = (a + a_size).min(b + b_size);
    (start, end - start)
}

fn replace_buffer_state(
    states: &mut Vec<BufferRangeState>,
    offset: u64,
    size: u64,
    state: ResourceState,
) {
    let end = offset + size;
    let mut next = Vec::new();
    for old in states.drain(..) {
        let old_end = old.offset + old.size;
        if old_end <= offset || end <= old.offset {
            next.push(old);
            continue;
        }
        if old.offset < offset {
            next.push(BufferRangeState {
                offset: old.offset,
                size: offset - old.offset,
                state: old.state,
                released_to: old.released_to,
            });
        }
        if end < old_end {
            next.push(BufferRangeState {
                offset: end,
                size: old_end - end,
                state: old.state,
                released_to: old.released_to,
            });
        }
    }
    next.push(BufferRangeState {
        offset,
        size,
        state: Some(state),
        released_to: None,
    });
    next.sort_by_key(|entry| entry.offset);
    *states = next;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rhi::{command::ImageCopyRegion, lifetime::LifetimeTracker};
    use std::sync::OnceLock;

    #[derive(Debug)]
    struct TestBuffer {
        id: ObjectId,
        size: u64,
    }

    impl TestBuffer {
        #[allow(clippy::new_ret_no_self)]
        fn new(size: u64) -> Buffer {
            Buffer::from_inner(Arc::new(Self {
                id: ObjectId::allocate(),
                size,
            }))
        }
    }

    impl RhiObject for TestBuffer {
        fn id(&self) -> ObjectId {
            self.id
        }
    }

    impl RhiBuffer for TestBuffer {
        fn name(&self) -> &str {
            "test"
        }

        fn size(&self) -> u64 {
            self.size
        }
    }

    #[derive(Debug)]
    struct TestImage {
        id: ObjectId,
        info: ImageSubresourceInfo,
    }

    impl TestImage {
        fn backend() -> Arc<dyn RhiImage> {
            Arc::new(Self {
                id: ObjectId::allocate(),
                info: ImageSubresourceInfo {
                    aspects: ImageAspectFlags::COLOR,
                    mip_levels: 4,
                    array_layers: 4,
                },
            })
        }

        #[allow(clippy::new_ret_no_self)]
        fn new() -> Image {
            Image::from_inner(Self::backend())
        }
    }

    impl RhiObject for TestImage {
        fn id(&self) -> ObjectId {
            self.id
        }
    }

    impl RhiImage for TestImage {
        fn subresource_info(&self) -> ImageSubresourceInfo {
            self.info
        }
    }

    #[derive(Debug)]
    struct TestImageView {
        id: ObjectId,
        image: Arc<dyn RhiImage>,
        range: ImageSubresourceRange,
    }

    impl RhiObject for TestImageView {
        fn id(&self) -> ObjectId {
            self.id
        }
    }

    impl RhiImageView for TestImageView {
        fn image(&self) -> &dyn RhiImage {
            self.image.as_ref()
        }

        fn subresource_range(&self) -> ImageSubresourceRange {
            self.range
        }
    }

    #[test]
    fn public_wrapper_queries_forward_to_backend_objects() {
        let buffer = TestBuffer::new(64);
        assert_eq!(buffer.name(), "test");
        assert_eq!(buffer.size(), 64);

        let image = TestImage::new();
        assert_eq!(
            image.subresource_info(),
            ImageSubresourceInfo {
                aspects: ImageAspectFlags::COLOR,
                mip_levels: 4,
                array_layers: 4,
            }
        );

        let range = ImageSubresourceRange::new(ImageAspect::Color, 1, 2, 0, 3).unwrap();
        let view = ImageView::from_inner(Arc::new(TestImageView {
            id: ObjectId::allocate(),
            image: TestImage::backend(),
            range,
        }));
        assert_eq!(view.subresource_range(), range);
    }

    #[test]
    fn image_subresource_range_rejects_invalid_bounds() {
        let info = ImageSubresourceInfo {
            aspects: ImageAspectFlags::COLOR,
            mip_levels: 4,
            array_layers: 2,
        };
        for range in [
            ImageSubresourceRange {
                aspect: ImageAspect::Color,
                base_mip: 0,
                mip_count: 0,
                base_layer: 0,
                layer_count: 1,
            },
            ImageSubresourceRange {
                aspect: ImageAspect::Color,
                base_mip: 0,
                mip_count: 1,
                base_layer: 0,
                layer_count: 0,
            },
            ImageSubresourceRange {
                aspect: ImageAspect::Color,
                base_mip: u32::MAX,
                mip_count: 2,
                base_layer: 0,
                layer_count: 1,
            },
            ImageSubresourceRange {
                aspect: ImageAspect::Color,
                base_mip: 3,
                mip_count: 2,
                base_layer: 0,
                layer_count: 1,
            },
            ImageSubresourceRange {
                aspect: ImageAspect::Color,
                base_mip: 0,
                mip_count: 1,
                base_layer: 1,
                layer_count: 2,
            },
            ImageSubresourceRange {
                aspect: ImageAspect::Depth,
                base_mip: 0,
                mip_count: 1,
                base_layer: 0,
                layer_count: 1,
            },
        ] {
            assert!(matches!(
                range.validate(info),
                Err(RhiError::InvalidCommand(_))
            ));
        }
    }

    #[test]
    fn disjoint_image_mips_each_get_an_initial_transition() {
        let tracker = Arc::new(ResourceTracker::new());
        let lifetime = Arc::new(LifetimeTracker::new());
        let source = TestImage::new();
        let destination = TestImage::new();
        let copy = |mip| Command::CopyImage {
            src: &source,
            src_region: ImageCopyRegion {
                subresource: ImageSubresourceLayers {
                    aspect: ImageAspect::Color,
                    mip,
                    base_layer: 0,
                    layer_count: 1,
                },
                offset: [0; 3],
            },
            dst: &destination,
            dst_region: ImageCopyRegion {
                subresource: ImageSubresourceLayers {
                    aspect: ImageAspect::Color,
                    mip,
                    base_layer: 0,
                    layer_count: 1,
                },
                offset: [0; 3],
            },
            extent: [1; 3],
        };
        let commands = [copy(0), copy(1)];

        let (planned, _transaction, _reservation) =
            plan(&tracker, &lifetime, graphics_queue(), &commands).unwrap();
        let has_initial_transition = |mip| {
            planned.iter().any(|operation| match operation {
                PlannedOp::Barriers(transitions) => transitions.iter().any(|transition| {
                    transition.resource.id() == destination.id()
                        && transition.before.is_none()
                        && transition.range
                        == ResourceRange::Image(ImageSubresourceRange {
                        aspect: ImageAspect::Color,
                        base_mip: mip,
                        mip_count: 1,
                        base_layer: 0,
                        layer_count: 1,
                    })
                }),
                PlannedOp::Command(_) => false,
            })
        };
        assert!(has_initial_transition(0));
        assert!(has_initial_transition(1));
    }

    fn graphics_queue() -> QueueId {
        static ID: OnceLock<QueueId> = OnceLock::new();
        *ID.get_or_init(QueueId::allocate)
    }

    fn transfer_queue() -> QueueId {
        static ID: OnceLock<QueueId> = OnceLock::new();
        *ID.get_or_init(QueueId::allocate)
    }

    fn resolve_queue(kind: QueueKind) -> RhiResult<QueueId> {
        match kind {
            QueueKind::Graphics => Ok(graphics_queue()),
            QueueKind::Transfer => Ok(transfer_queue()),
        }
    }

    fn plan<'a>(
        tracker: &Arc<ResourceTracker>,
        lifetime: &Arc<LifetimeTracker>,
        queue: QueueId,
        commands: &[Command<'a>],
    ) -> RhiResult<(Vec<PlannedOp<'a>>, TrackerTransaction, LifetimeReservation)> {
        let reservation = lifetime.reserve_commands(commands)?;
        let (planned, transaction) = tracker.plan(queue, commands, &reservation, resolve_queue)?;
        Ok((planned, transaction, reservation))
    }

    #[test]
    fn planner_preserves_commands_and_inserts_write_dependencies() {
        let tracker = Arc::new(ResourceTracker::new());
        let lifetime = Arc::new(LifetimeTracker::new());
        let a = TestBuffer::new(64);
        let b = TestBuffer::new(64);
        let commands = [
            Command::CopyBuffer {
                src: &a,
                src_offset: 0,
                dst: &b,
                dst_offset: 0,
                size: 64,
            },
            Command::CopyBuffer {
                src: &b,
                src_offset: 0,
                dst: &a,
                dst_offset: 0,
                size: 64,
            },
        ];

        let (planned, _transaction, _reservation) =
            plan(&tracker, &lifetime, graphics_queue(), &commands).unwrap();
        assert!(matches!(
            planned[0],
            PlannedOp::Command(Command::CopyBuffer { .. })
        ));
        let PlannedOp::Barriers(barriers) = &planned[1] else {
            panic!("second copy must be preceded by barriers");
        };
        assert_eq!(barriers.len(), 2);
        assert!(matches!(
            planned[2],
            PlannedOp::Command(Command::CopyBuffer { .. })
        ));
    }

    #[test]
    fn disjoint_buffer_ranges_keep_independent_state() {
        let queue = graphics_queue();
        let mut states = vec![BufferRangeState {
            offset: 0,
            size: 64,
            state: None,
            released_to: None,
        }];
        replace_buffer_state(
            &mut states,
            0,
            16,
            ResourceState {
                access: Access::TransferWrite,
                queue,
            },
        );
        assert_eq!(states.len(), 2);
        assert_eq!(
            states
                .iter()
                .find(|entry| entry.offset == 16 && entry.size == 48)
                .unwrap()
                .state,
            None
        );
    }

    #[test]
    fn tracker_only_commits_after_successful_submit_point() {
        let tracker = Arc::new(ResourceTracker::new());
        let lifetime = Arc::new(LifetimeTracker::new());
        let a = TestBuffer::new(16);
        let b = TestBuffer::new(16);
        let commands = [Command::CopyBuffer {
            src: &a,
            src_offset: 0,
            dst: &b,
            dst_offset: 0,
            size: 16,
        }];

        let (_, transaction, reservation) =
            plan(&tracker, &lifetime, graphics_queue(), &commands).unwrap();
        assert!(lifetime.last_uses(a.id()).is_empty());
        drop(transaction);
        drop(reservation);
        assert!(lifetime.last_uses(a.id()).is_empty());

        let (_, transaction, reservation) =
            plan(&tracker, &lifetime, graphics_queue(), &commands).unwrap();
        let point = SyncPoint::new(graphics_queue(), 7, 3);
        transaction.commit();
        reservation.commit(point);
        assert_eq!(lifetime.last_uses(a.id()), vec![point]);
        assert_eq!(lifetime.last_uses(b.id()), vec![point]);

        let next = SyncPoint::new(graphics_queue(), 9, 4);
        lifetime.reserve([a.id(), b.id()]).unwrap().commit(next);
        assert_eq!(lifetime.last_uses(a.id()), vec![next]);
        assert_eq!(lifetime.last_uses(b.id()), vec![next]);
    }

    #[test]
    fn pending_submission_reserves_resources() {
        let tracker = Arc::new(ResourceTracker::new());
        let lifetime = Arc::new(LifetimeTracker::new());
        let a = TestBuffer::new(16);
        let b = TestBuffer::new(16);
        let commands = [Command::CopyBuffer {
            src: &a,
            src_offset: 0,
            dst: &b,
            dst_offset: 0,
            size: 16,
        }];
        let (_, transaction, reservation) =
            plan(&tracker, &lifetime, graphics_queue(), &commands).unwrap();
        assert!(plan(&tracker, &lifetime, graphics_queue(), &commands).is_err());
        drop(transaction);
        drop(reservation);
        assert!(plan(&tracker, &lifetime, graphics_queue(), &commands).is_ok());
    }

    #[test]
    fn invalid_command_structure_is_rejected() {
        let tracker = Arc::new(ResourceTracker::new());
        let lifetime = Arc::new(LifetimeTracker::new());
        let commands = [Command::EndRendering];
        assert!(matches!(
            plan(&tracker, &lifetime, graphics_queue(), &commands),
            Err(RhiError::InvalidCommand(_))
        ));
    }

    #[test]
    fn transfer_release_becomes_graphics_wait_and_acquire() {
        let tracker = Arc::new(ResourceTracker::new());
        let lifetime = Arc::new(LifetimeTracker::new());
        let staging = TestBuffer::new(16);
        let uploaded = TestBuffer::new(16);
        let output = TestBuffer::new(16);
        let released = [&uploaded];
        let transfer_commands = [
            Command::CopyBuffer {
                src: &staging,
                src_offset: 0,
                dst: &uploaded,
                dst_offset: 0,
                size: 16,
            },
            Command::ReleaseBuffers {
                buffers: &released,
                target: QueueKind::Graphics,
            },
        ];
        let (_, transfer_transaction, transfer_reservation) =
            plan(&tracker, &lifetime, transfer_queue(), &transfer_commands).unwrap();
        let transfer_point = SyncPoint::new(transfer_queue(), 11, 4);
        transfer_transaction.commit();
        transfer_reservation.commit(transfer_point);

        let graphics_commands = [Command::CopyBuffer {
            src: &uploaded,
            src_offset: 0,
            dst: &output,
            dst_offset: 0,
            size: 16,
        }];
        let (planned, graphics_transaction, _graphics_reservation) =
            plan(&tracker, &lifetime, graphics_queue(), &graphics_commands).unwrap();
        assert_eq!(graphics_transaction.waits, vec![transfer_point]);
        let PlannedOp::Barriers(transitions) = &planned[0] else {
            panic!("graphics use must acquire transfer ownership");
        };
        assert_eq!(transitions[0].kind, TransitionKind::Acquire);
        assert!(transitions
            .iter()
            .any(|transition| transition.kind == TransitionKind::Memory));
    }

    #[test]
    fn partial_use_acquires_the_complete_released_buffer_range() {
        let tracker = Arc::new(ResourceTracker::new());
        let lifetime = Arc::new(LifetimeTracker::new());
        let staging = TestBuffer::new(64);
        let uploaded = TestBuffer::new(64);
        let output = TestBuffer::new(16);
        let released = [&uploaded];
        let transfer_commands = [
            Command::CopyBuffer {
                src: &staging,
                src_offset: 0,
                dst: &uploaded,
                dst_offset: 0,
                size: 64,
            },
            Command::ReleaseBuffers {
                buffers: &released,
                target: QueueKind::Graphics,
            },
        ];
        let (transfer_plan, transfer_transaction, transfer_reservation) =
            plan(&tracker, &lifetime, transfer_queue(), &transfer_commands).unwrap();
        let release = transfer_plan
            .iter()
            .filter_map(|operation| match operation {
                PlannedOp::Barriers(transitions) => transitions
                    .iter()
                    .find(|transition| {
                        transition.kind == TransitionKind::Release
                            && transition.resource.id() == uploaded.id()
                    })
                    .copied(),
                PlannedOp::Command(_) => None,
            })
            .next()
            .expect("transfer plan must release the uploaded buffer");
        assert_eq!(
            release.range,
            ResourceRange::Buffer {
                offset: 0,
                size: 64
            }
        );
        let transfer_point = SyncPoint::new(transfer_queue(), 17, 3);
        transfer_transaction.commit();
        transfer_reservation.commit(transfer_point);

        let graphics_commands = [Command::CopyBuffer {
            src: &uploaded,
            src_offset: 16,
            dst: &output,
            dst_offset: 0,
            size: 16,
        }];
        let (graphics_plan, _graphics_transaction, _graphics_reservation) =
            plan(&tracker, &lifetime, graphics_queue(), &graphics_commands).unwrap();
        let transitions: Vec<_> = graphics_plan
            .iter()
            .filter_map(|operation| match operation {
                PlannedOp::Barriers(transitions) => Some(transitions.as_slice()),
                PlannedOp::Command(_) => None,
            })
            .flatten()
            .filter(|transition| transition.resource.id() == uploaded.id())
            .collect();
        let acquire = transitions
            .iter()
            .find(|transition| transition.kind == TransitionKind::Acquire)
            .expect("partial use must acquire ownership");
        assert_eq!(acquire.range, release.range);
        let memory = transitions
            .iter()
            .find(|transition| transition.kind == TransitionKind::Memory)
            .expect("transfer write to transfer read needs a memory transition");
        assert_eq!(
            memory.range,
            ResourceRange::Buffer {
                offset: 16,
                size: 16
            }
        );
    }

    #[test]
    fn image_acquire_matches_its_released_subresource_state() {
        let tracker = Arc::new(ResourceTracker::new());
        let lifetime = Arc::new(LifetimeTracker::new());
        let source = TestImage::new();
        let uploaded = TestImage::new();
        let output = TestImage::new();
        let released = [&uploaded];
        let two_layers = ImageSubresourceLayers {
            aspect: ImageAspect::Color,
            mip: 0,
            base_layer: 0,
            layer_count: 2,
        };
        let transfer_commands = [
            Command::CopyImage {
                src: &source,
                src_region: ImageCopyRegion {
                    subresource: two_layers,
                    offset: [0; 3],
                },
                dst: &uploaded,
                dst_region: ImageCopyRegion {
                    subresource: two_layers,
                    offset: [0; 3],
                },
                extent: [1; 3],
            },
            Command::ReleaseImages {
                images: &released,
                target: QueueKind::Graphics,
            },
        ];
        let (transfer_plan, transfer_transaction, transfer_reservation) =
            plan(&tracker, &lifetime, transfer_queue(), &transfer_commands).unwrap();
        let transfer_point = SyncPoint::new(transfer_queue(), 19, 2);
        transfer_transaction.commit();
        transfer_reservation.commit(transfer_point);

        let one_layer = ImageSubresourceLayers {
            layer_count: 1,
            ..two_layers
        };
        let graphics_commands = [Command::CopyImage {
            src: &uploaded,
            src_region: ImageCopyRegion {
                subresource: one_layer,
                offset: [0; 3],
            },
            dst: &output,
            dst_region: ImageCopyRegion {
                subresource: one_layer,
                offset: [0; 3],
            },
            extent: [1; 3],
        }];
        let (graphics_plan, _graphics_transaction, _graphics_reservation) =
            plan(&tracker, &lifetime, graphics_queue(), &graphics_commands).unwrap();
        let acquire = graphics_plan
            .iter()
            .filter_map(|operation| match operation {
                PlannedOp::Barriers(transitions) => transitions.iter().find(|transition| {
                    transition.kind == TransitionKind::Acquire
                        && transition.resource.id() == uploaded.id()
                }),
                PlannedOp::Command(_) => None,
            })
            .next()
            .expect("partial image use must acquire its released state");
        let matching_release = transfer_plan.iter().any(|operation| match operation {
            PlannedOp::Barriers(transitions) => transitions.iter().any(|transition| {
                transition.kind == TransitionKind::Release
                    && transition.resource.id() == uploaded.id()
                    && transition.range == acquire.range
            }),
            PlannedOp::Command(_) => false,
        });
        assert!(
            matching_release,
            "acquire must exactly match a release range"
        );
    }

    #[test]
    fn release_command_keeps_position_and_batches_resources() {
        let tracker = Arc::new(ResourceTracker::new());
        let lifetime = Arc::new(LifetimeTracker::new());
        let staging = TestBuffer::new(32);
        let first = TestBuffer::new(16);
        let second = TestBuffer::new(16);
        let released = [&first, &second];
        let commands = [
            Command::CopyBuffer {
                src: &staging,
                src_offset: 0,
                dst: &first,
                dst_offset: 0,
                size: 16,
            },
            Command::CopyBuffer {
                src: &staging,
                src_offset: 16,
                dst: &second,
                dst_offset: 0,
                size: 16,
            },
            Command::ReleaseBuffers {
                buffers: &released,
                target: QueueKind::Graphics,
            },
        ];

        let (planned, _transaction, _reservation) =
            plan(&tracker, &lifetime, transfer_queue(), &commands).unwrap();
        assert!(matches!(
            planned[0],
            PlannedOp::Command(Command::CopyBuffer { .. })
        ));
        assert!(matches!(
            planned[1],
            PlannedOp::Command(Command::CopyBuffer { .. })
        ));
        let PlannedOp::Barriers(releases) = &planned[2] else {
            panic!("release must remain after both copies");
        };
        assert_eq!(releases.len(), 2);
        assert!(releases
            .iter()
            .all(|transition| transition.kind == TransitionKind::Release));
    }

    #[test]
    fn invalid_release_does_not_leave_resources_reserved() {
        let tracker = Arc::new(ResourceTracker::new());
        let lifetime = Arc::new(LifetimeTracker::new());
        let staging = TestBuffer::new(16);
        let uploaded = TestBuffer::new(16);
        let released = [&uploaded];
        let invalid = [
            Command::CopyBuffer {
                src: &staging,
                src_offset: 0,
                dst: &uploaded,
                dst_offset: 0,
                size: 16,
            },
            Command::ReleaseBuffers {
                buffers: &released,
                target: QueueKind::Transfer,
            },
        ];
        assert!(matches!(
            plan(&tracker, &lifetime, transfer_queue(), &invalid),
            Err(RhiError::ResourceState(_))
        ));

        let valid = [Command::CopyBuffer {
            src: &staging,
            src_offset: 0,
            dst: &uploaded,
            dst_offset: 0,
            size: 16,
        }];
        assert!(plan(&tracker, &lifetime, transfer_queue(), &valid).is_ok());
    }

    #[test]
    fn empty_and_duplicate_release_lists_are_rejected() {
        let tracker = Arc::new(ResourceTracker::new());
        let lifetime = Arc::new(LifetimeTracker::new());
        let buffer = TestBuffer::new(16);
        let empty: [&Buffer; 0] = [];
        let duplicates = [&buffer, &buffer];

        assert!(matches!(
            plan(
                &tracker,
                &lifetime,
                transfer_queue(),
                &[Command::ReleaseBuffers {
                    buffers: &empty,
                    target: QueueKind::Graphics,
                }],
            ),
            Err(RhiError::InvalidCommand(_))
        ));
        assert!(matches!(
            plan(
                &tracker,
                &lifetime,
                transfer_queue(),
                &[Command::ReleaseBuffers {
                    buffers: &duplicates,
                    target: QueueKind::Graphics,
                }],
            ),
            Err(RhiError::InvalidCommand(_))
        ));
    }
}
