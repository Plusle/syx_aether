#![allow(dead_code)]

use crate::rhi::{
    command::{Command, ImageSubresourceLayers},
    error::{RhiError, RhiResult},
    queue::{QueueId, QueueKind},
    sync::SyncPoint,
    types::RhiType,
};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};

/// Stable identity used by command planning and deferred destruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ResourceId(u64);

impl ResourceId {
    pub(crate) fn allocate() -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        Self(NEXT_ID.fetch_add(1, Ordering::Relaxed))
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

/// One image aspect selected by an operation or view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImageAspect {
    Color,
    Depth,
    Stencil,
}

/// Backend-neutral image subresource range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ImageSubresourceRange {
    pub aspect: ImageAspect,
    pub base_mip: u32,
    pub mip_count: u32,
    pub base_layer: u32,
    pub layer_count: u32,
}

pub trait RhiBuffer: RhiType {
    fn id(&self) -> ResourceId;
    fn name(&self) -> &str;
    fn size(&self) -> u64;
}

pub trait RhiImage: RhiType {
    fn id(&self) -> ResourceId;
}

pub trait RhiSampler: RhiType {}

pub trait RhiImageView: RhiType {
    fn image_id(&self) -> ResourceId;
    fn subresource_range(&self) -> ImageSubresourceRange;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceRange {
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
    ImageView(&'a dyn RhiImageView),
}

impl ResourceRef<'_> {
    pub(crate) fn id(self) -> ResourceId {
        match self {
            Self::Buffer(resource) => resource.id(),
            Self::Image(resource) => resource.id(),
            Self::ImageView(resource) => resource.image_id(),
        }
    }

    fn is_image(self) -> bool {
        !matches!(self, Self::Buffer(_))
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
    pub(crate) range: ResourceRange,
    pub(crate) access: Access,
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
struct RangeState {
    range: ResourceRange,
    state: Option<ResourceState>,
    released_to: Option<QueueId>,
}

#[derive(Debug, Default)]
struct TrackedResource {
    states: Vec<RangeState>,
    last_use: Option<SyncPoint>,
    pending: u32,
    retired: bool,
}

/// Device-global scheduled resource state. GPU-completed state remains the
/// responsibility of queue timelines and the future discard pool.
#[derive(Debug, Default)]
pub struct ResourceTracker {
    resources: Mutex<HashMap<ResourceId, TrackedResource>>,
}

impl ResourceTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn last_use(&self, id: ResourceId) -> Option<SyncPoint> {
        self.resources
            .lock()
            .expect("resource tracker poisoned")
            .get(&id)
            .and_then(|resource| resource.last_use)
    }

    pub fn retire(&self, id: ResourceId) {
        if let Some(resource) = self
            .resources
            .lock()
            .expect("resource tracker poisoned")
            .get_mut(&id)
        {
            resource.retired = true;
        }
    }

    pub fn can_destroy(&self, id: ResourceId, completed: impl Fn(SyncPoint) -> bool) -> bool {
        let resources = self.resources.lock().expect("resource tracker poisoned");
        let Some(resource) = resources.get(&id) else {
            return true;
        };
        resource.retired
            && resource.pending == 0
            && resource.last_use.map(&completed).unwrap_or(true)
    }

    pub(crate) fn forget(&self, id: ResourceId) {
        self.resources
            .lock()
            .expect("resource tracker poisoned")
            .remove(&id);
    }

    pub(crate) fn plan<'a>(
        self: &Arc<Self>,
        queue: QueueId,
        commands: &[Command<'a>],
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
        let mut resources = self
            .resources
            .lock()
            .map_err(|_| RhiError::ResourceState("resource tracker poisoned".into()))?;

        if let Some(id) = touched.iter().find(|id| {
            resources
                .get(id)
                .is_some_and(|resource| resource.pending != 0)
        }) {
            return Err(RhiError::ResourceState(format!(
                "resource {} is already reserved by another pending submission",
                id.get()
            )));
        }

        for id in &touched {
            resources.entry(*id).or_default().pending += 1;
        }

        let mut local: HashMap<_, Vec<RangeState>> = touched
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
        let last_uses: HashMap<_, _> = touched
            .iter()
            .filter_map(|id| {
                resources
                    .get(id)
                    .and_then(|resource| resource.last_use.map(|point| (*id, point)))
            })
            .collect();
        drop(resources);

        // From this point on, every early error must release the reservations.
        let mut transaction = TrackerTransaction {
            tracker: Arc::clone(self),
            touched: touched.into_iter().collect(),
            states: HashMap::new(),
            waits: Vec::new(),
            committed: false,
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
                    let mut released = false;
                    for current in states.iter_mut() {
                        if current.released_to.is_some() {
                            return Err(RhiError::ResourceState(format!(
                                "resource {} has already been released",
                                resource.id().get()
                            )));
                        }
                        let Some(before) = current.state else {
                            continue;
                        };
                        if before.queue != queue {
                            return Err(RhiError::ResourceState(
                                "only the owning queue can release a resource".into(),
                            ));
                        }
                        releases.push(Transition {
                            resource,
                            range: current.range,
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
                ensure_initial_state(states, usage);
                let mut did_acquire = false;
                let after = ResourceState {
                    access: usage.access,
                    queue,
                };
                let overlapping: Vec<_> = states
                    .iter()
                    .filter(|state| overlaps(state.range, usage.range))
                    .copied()
                    .collect();
                for current in overlapping {
                    if let Some(target) = current.released_to {
                        if target != queue {
                            return Err(RhiError::ResourceState(format!(
                                "resource {} was released to a different queue",
                                id.get()
                            )));
                        }
                        let before = current.state.expect("released resources have a state");
                        let acquired_state = ResourceState {
                            access: before.access,
                            queue,
                        };
                        transitions.push(Transition {
                            resource: usage.resource,
                            // Vulkan requires acquire to exactly match the
                            // range of its paired release. Acquiring any part
                            // of this state therefore acquires the whole
                            // released state before applying the narrower use.
                            range: current.range,
                            before: Some(before),
                            after: acquired_state,
                            kind: TransitionKind::Acquire,
                        });
                        replace_state(states, current.range, acquired_state);
                        did_acquire = true;
                        if needs_transition(Some(acquired_state), after, usage.resource.is_image())
                        {
                            transitions.push(Transition {
                                resource: usage.resource,
                                range: intersection(current.range, usage.range),
                                before: Some(acquired_state),
                                after,
                                kind: TransitionKind::Memory,
                            });
                        }
                        continue;
                    }
                    if current.state.is_some_and(|before| before.queue != queue) {
                        return Err(RhiError::ResourceState(format!(
                            "resource {} crossed queues without an ownership release",
                            id.get()
                        )));
                    }
                    if needs_transition(current.state, after, usage.resource.is_image()) {
                        transitions.push(Transition {
                            resource: usage.resource,
                            range: intersection(current.range, usage.range),
                            before: current.state,
                            after,
                            kind: TransitionKind::Memory,
                        });
                    }
                }
                replace_state(states, usage.range, after);
                if did_acquire {
                    if let Some(point) = last_uses.get(&id) {
                        waits.insert(*point);
                    }
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
    touched: Vec<ResourceId>,
    states: HashMap<ResourceId, Vec<RangeState>>,
    pub(crate) waits: Vec<SyncPoint>,
    committed: bool,
}

impl TrackerTransaction {
    pub(crate) fn commit(mut self, point: SyncPoint) {
        let mut resources = self
            .tracker
            .resources
            .lock()
            .expect("resource tracker poisoned");
        for id in &self.touched {
            let resource = resources.entry(*id).or_default();
            resource.states = self.states.remove(id).unwrap_or_default();
            resource.last_use = Some(point);
            resource.pending -= 1;
        }
        self.committed = true;
    }
}

impl Drop for TrackerTransaction {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        let mut resources = self
            .tracker
            .resources
            .lock()
            .expect("resource tracker poisoned");
        for id in &self.touched {
            if let Some(resource) = resources.get_mut(id) {
                resource.pending -= 1;
            }
        }
    }
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
                    uses.push(ResourceUse {
                        resource: ResourceRef::ImageView(attachment.view),
                        range: ResourceRange::Image(attachment.view.subresource_range()),
                        access: Access::ColorAttachmentReadWrite,
                    });
                    if let Some(resolve) = attachment.resolve {
                        uses.push(ResourceUse {
                            resource: ResourceRef::ImageView(resolve),
                            range: ResourceRange::Image(resolve.subresource_range()),
                            access: Access::ColorAttachmentReadWrite,
                        });
                    }
                }
                if let Some(attachment) = desc.depth_stencil {
                    uses.push(ResourceUse {
                        resource: ResourceRef::ImageView(attachment.view),
                        range: ResourceRange::Image(attachment.view.subresource_range()),
                        access: Access::DepthStencilReadWrite,
                    });
                    if let Some(resolve) = attachment.resolve {
                        uses.push(ResourceUse {
                            resource: ResourceRef::ImageView(resolve),
                            range: ResourceRange::Image(resolve.subresource_range()),
                            access: Access::DepthStencilReadWrite,
                        });
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
                ),
            ],
            Command::CopyImageToBuffer { src, dst, region } => vec![
                image_use(
                    ResourceRef::Image(src),
                    region.image_subresource,
                    Access::TransferRead,
                ),
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
                ),
                image_use(
                    ResourceRef::Image(dst),
                    dst_region.subresource,
                    Access::TransferWrite,
                ),
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
        range: ResourceRange::Buffer { offset, size },
        access,
    })
}

fn image_use(
    resource: ResourceRef<'_>,
    layers: ImageSubresourceLayers,
    access: Access,
) -> ResourceUse<'_> {
    ResourceUse {
        resource,
        range: ResourceRange::Image(ImageSubresourceRange {
            aspect: layers.aspect,
            base_mip: layers.mip,
            mip_count: 1,
            base_layer: layers.base_layer,
            layer_count: layers.layer_count,
        }),
        access,
    }
}

fn ensure_initial_state(states: &mut Vec<RangeState>, usage: ResourceUse<'_>) {
    if !states.is_empty() {
        return;
    }
    let range = match usage.resource {
        ResourceRef::Buffer(buffer) => ResourceRange::Buffer {
            offset: 0,
            size: buffer.size(),
        },
        ResourceRef::Image(_) | ResourceRef::ImageView(_) => usage.range,
    };
    states.push(RangeState {
        range,
        state: None,
        released_to: None,
    });
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

fn overlaps(a: ResourceRange, b: ResourceRange) -> bool {
    match (a, b) {
        (
            ResourceRange::Buffer {
                offset: a,
                size: a_size,
            },
            ResourceRange::Buffer {
                offset: b,
                size: b_size,
            },
        ) => a < b.saturating_add(b_size) && b < a.saturating_add(a_size),
        (ResourceRange::Image(a), ResourceRange::Image(b)) => {
            a.aspect == b.aspect
                && a.base_mip < b.base_mip.saturating_add(b.mip_count)
                && b.base_mip < a.base_mip.saturating_add(a.mip_count)
                && a.base_layer < b.base_layer.saturating_add(b.layer_count)
                && b.base_layer < a.base_layer.saturating_add(a.layer_count)
        }
        _ => false,
    }
}

fn intersection(a: ResourceRange, b: ResourceRange) -> ResourceRange {
    match (a, b) {
        (
            ResourceRange::Buffer {
                offset: a,
                size: a_size,
            },
            ResourceRange::Buffer {
                offset: b,
                size: b_size,
            },
        ) => {
            let start = a.max(b);
            let end = a.saturating_add(a_size).min(b.saturating_add(b_size));
            ResourceRange::Buffer {
                offset: start,
                size: end - start,
            }
        }
        (ResourceRange::Image(a), ResourceRange::Image(b)) => {
            let mip = a.base_mip.max(b.base_mip);
            let mip_end = a
                .base_mip
                .saturating_add(a.mip_count)
                .min(b.base_mip.saturating_add(b.mip_count));
            let layer = a.base_layer.max(b.base_layer);
            let layer_end = a
                .base_layer
                .saturating_add(a.layer_count)
                .min(b.base_layer.saturating_add(b.layer_count));
            ResourceRange::Image(ImageSubresourceRange {
                aspect: a.aspect,
                base_mip: mip,
                mip_count: mip_end - mip,
                base_layer: layer,
                layer_count: layer_end - layer,
            })
        }
        _ => unreachable!("resource range kinds must match"),
    }
}

fn replace_state(states: &mut Vec<RangeState>, range: ResourceRange, state: ResourceState) {
    match range {
        ResourceRange::Buffer { offset, size } => {
            let end = offset + size;
            let mut next = Vec::new();
            for old in states.drain(..) {
                let ResourceRange::Buffer {
                    offset: old_offset,
                    size: old_size,
                } = old.range
                else {
                    next.push(old);
                    continue;
                };
                let old_end = old_offset + old_size;
                if old_end <= offset || end <= old_offset {
                    next.push(old);
                    continue;
                }
                if old_offset < offset {
                    next.push(RangeState {
                        range: ResourceRange::Buffer {
                            offset: old_offset,
                            size: offset - old_offset,
                        },
                        state: old.state,
                        released_to: old.released_to,
                    });
                }
                if end < old_end {
                    next.push(RangeState {
                        range: ResourceRange::Buffer {
                            offset: end,
                            size: old_end - end,
                        },
                        state: old.state,
                        released_to: old.released_to,
                    });
                }
            }
            next.push(RangeState {
                range,
                state: Some(state),
                released_to: None,
            });
            next.sort_by_key(|entry| match entry.range {
                ResourceRange::Buffer { offset, .. } => offset,
                ResourceRange::Image(_) => u64::MAX,
            });
            *states = next;
        }
        ResourceRange::Image(range) => {
            let mut next = Vec::new();
            for mip in range.base_mip..range.base_mip + range.mip_count {
                for layer in range.base_layer..range.base_layer + range.layer_count {
                    let cell = ResourceRange::Image(ImageSubresourceRange {
                        aspect: range.aspect,
                        base_mip: mip,
                        mip_count: 1,
                        base_layer: layer,
                        layer_count: 1,
                    });
                    states.retain(|old| !overlaps(old.range, cell));
                    next.push(RangeState {
                        range: cell,
                        state: Some(state),
                        released_to: None,
                    });
                }
            }
            states.extend(next);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rhi::command::ImageCopyRegion;
    use std::sync::OnceLock;

    #[derive(Debug)]
    struct TestBuffer {
        id: ResourceId,
        size: u64,
    }

    impl TestBuffer {
        fn new(size: u64) -> Self {
            Self {
                id: ResourceId::allocate(),
                size,
            }
        }
    }

    impl RhiBuffer for TestBuffer {
        fn id(&self) -> ResourceId {
            self.id
        }

        fn name(&self) -> &str {
            "test"
        }

        fn size(&self) -> u64 {
            self.size
        }
    }

    #[derive(Debug)]
    struct TestImage {
        id: ResourceId,
    }

    impl TestImage {
        fn new() -> Self {
            Self {
                id: ResourceId::allocate(),
            }
        }
    }

    impl RhiImage for TestImage {
        fn id(&self) -> ResourceId {
            self.id
        }
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

    #[test]
    fn planner_preserves_commands_and_inserts_write_dependencies() {
        let tracker = Arc::new(ResourceTracker::new());
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

        let (planned, _transaction) = tracker
            .plan(graphics_queue(), &commands, resolve_queue)
            .unwrap();
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
        let mut states = vec![RangeState {
            range: ResourceRange::Buffer {
                offset: 0,
                size: 64,
            },
            state: None,
            released_to: None,
        }];
        replace_state(
            &mut states,
            ResourceRange::Buffer {
                offset: 0,
                size: 16,
            },
            ResourceState {
                access: Access::TransferWrite,
                queue,
            },
        );
        assert_eq!(states.len(), 2);
        assert_eq!(
            states
                .iter()
                .find(|entry| {
                    entry.range
                        == ResourceRange::Buffer {
                            offset: 16,
                            size: 48,
                        }
                })
                .unwrap()
                .state,
            None
        );
    }

    #[test]
    fn tracker_only_commits_after_successful_submit_point() {
        let tracker = Arc::new(ResourceTracker::new());
        let a = TestBuffer::new(16);
        let b = TestBuffer::new(16);
        let commands = [Command::CopyBuffer {
            src: &a,
            src_offset: 0,
            dst: &b,
            dst_offset: 0,
            size: 16,
        }];

        let (_, transaction) = tracker
            .plan(graphics_queue(), &commands, resolve_queue)
            .unwrap();
        assert_eq!(tracker.last_use(a.id()), None);
        drop(transaction);
        assert_eq!(tracker.last_use(a.id()), None);

        let (_, transaction) = tracker
            .plan(graphics_queue(), &commands, resolve_queue)
            .unwrap();
        let point = SyncPoint::new(7, 3);
        transaction.commit(point);
        assert_eq!(tracker.last_use(a.id()), Some(point));
        assert_eq!(tracker.last_use(b.id()), Some(point));
    }

    #[test]
    fn pending_submission_reserves_resources() {
        let tracker = Arc::new(ResourceTracker::new());
        let a = TestBuffer::new(16);
        let b = TestBuffer::new(16);
        let commands = [Command::CopyBuffer {
            src: &a,
            src_offset: 0,
            dst: &b,
            dst_offset: 0,
            size: 16,
        }];
        let (_, transaction) = tracker
            .plan(graphics_queue(), &commands, resolve_queue)
            .unwrap();
        assert!(tracker
            .plan(graphics_queue(), &commands, resolve_queue)
            .is_err());
        drop(transaction);
        assert!(tracker
            .plan(graphics_queue(), &commands, resolve_queue)
            .is_ok());
    }

    #[test]
    fn invalid_command_structure_is_rejected() {
        let tracker = Arc::new(ResourceTracker::new());
        let commands = [Command::EndRendering];
        assert!(matches!(
            tracker.plan(graphics_queue(), &commands, resolve_queue),
            Err(RhiError::InvalidCommand(_))
        ));
    }

    #[test]
    fn transfer_release_becomes_graphics_wait_and_acquire() {
        let tracker = Arc::new(ResourceTracker::new());
        let staging = TestBuffer::new(16);
        let uploaded = TestBuffer::new(16);
        let output = TestBuffer::new(16);
        let released: [&dyn RhiBuffer; 1] = [&uploaded];
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
        let (_, transfer_transaction) = tracker
            .plan(transfer_queue(), &transfer_commands, resolve_queue)
            .unwrap();
        let transfer_point = SyncPoint::new(11, 4);
        transfer_transaction.commit(transfer_point);

        let graphics_commands = [Command::CopyBuffer {
            src: &uploaded,
            src_offset: 0,
            dst: &output,
            dst_offset: 0,
            size: 16,
        }];
        let (planned, graphics_transaction) = tracker
            .plan(graphics_queue(), &graphics_commands, resolve_queue)
            .unwrap();
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
        let staging = TestBuffer::new(64);
        let uploaded = TestBuffer::new(64);
        let output = TestBuffer::new(16);
        let released: [&dyn RhiBuffer; 1] = [&uploaded];
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
        let (transfer_plan, transfer_transaction) = tracker
            .plan(transfer_queue(), &transfer_commands, resolve_queue)
            .unwrap();
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
        transfer_transaction.commit(SyncPoint::new(17, 3));

        let graphics_commands = [Command::CopyBuffer {
            src: &uploaded,
            src_offset: 16,
            dst: &output,
            dst_offset: 0,
            size: 16,
        }];
        let (graphics_plan, _graphics_transaction) = tracker
            .plan(graphics_queue(), &graphics_commands, resolve_queue)
            .unwrap();
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
        let source = TestImage::new();
        let uploaded = TestImage::new();
        let output = TestImage::new();
        let released: [&dyn RhiImage; 1] = [&uploaded];
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
        let (transfer_plan, transfer_transaction) = tracker
            .plan(transfer_queue(), &transfer_commands, resolve_queue)
            .unwrap();
        transfer_transaction.commit(SyncPoint::new(19, 2));

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
        let (graphics_plan, _graphics_transaction) = tracker
            .plan(graphics_queue(), &graphics_commands, resolve_queue)
            .unwrap();
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
        let staging = TestBuffer::new(32);
        let first = TestBuffer::new(16);
        let second = TestBuffer::new(16);
        let released: [&dyn RhiBuffer; 2] = [&first, &second];
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

        let (planned, _transaction) = tracker
            .plan(transfer_queue(), &commands, resolve_queue)
            .unwrap();
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
        let staging = TestBuffer::new(16);
        let uploaded = TestBuffer::new(16);
        let released: [&dyn RhiBuffer; 1] = [&uploaded];
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
            tracker.plan(transfer_queue(), &invalid, resolve_queue),
            Err(RhiError::ResourceState(_))
        ));

        let valid = [Command::CopyBuffer {
            src: &staging,
            src_offset: 0,
            dst: &uploaded,
            dst_offset: 0,
            size: 16,
        }];
        assert!(tracker
            .plan(transfer_queue(), &valid, resolve_queue)
            .is_ok());
    }

    #[test]
    fn empty_and_duplicate_release_lists_are_rejected() {
        let tracker = Arc::new(ResourceTracker::new());
        let buffer = TestBuffer::new(16);
        let empty: [&dyn RhiBuffer; 0] = [];
        let duplicates: [&dyn RhiBuffer; 2] = [&buffer, &buffer];

        assert!(matches!(
            tracker.plan(
                transfer_queue(),
                &[Command::ReleaseBuffers {
                    buffers: &empty,
                    target: QueueKind::Graphics,
                }],
                resolve_queue,
            ),
            Err(RhiError::InvalidCommand(_))
        ));
        assert!(matches!(
            tracker.plan(
                transfer_queue(),
                &[Command::ReleaseBuffers {
                    buffers: &duplicates,
                    target: QueueKind::Graphics,
                }],
                resolve_queue,
            ),
            Err(RhiError::InvalidCommand(_))
        ));
    }
}
