# syx_aether RHI design

| Field | Value |
|---|---|
| Status | Proposed design for discussion; not yet implemented |
| Scope | `src/rhi/**`; client-layer notes are marked `src/render/**` |
| Current baseline | Vulkan 1.3 + dynamic rendering + synchronization2 + timeline semaphores; ash 0.38, VMA, Slang; 45 tests passing |
| Primary input | [BLENDER-DESIGN-LESSONS.md](BLENDER-DESIGN-LESSONS.md), backed by the [Blender GPU study](../blender-gpu/README.md) |
| Non-goals | OpenGL compatibility, Blender API emulation, a dummy backend, a render graph inside the RHI |

This document is our design, not a summary of Blender. It records the choices we make for `syx_aether`, the reasons behind them, and the API shape that follows. The Blender study is evidence, not a template: Blender's public GPU API is shaped by two decades of OpenGL, and we do not carry that constraint.

## Contents

- [1. Thesis](#1-thesis)
- [2. Decisions at a glance](#2-decisions-at-a-glance)
- [3. Object and ownership model](#3-object-and-ownership-model)
- [4. Device and backend selection](#4-device-and-backend-selection)
- [5. Command recording and submission](#5-command-recording-and-submission)
- [6. Synchronization and barriers](#6-synchronization-and-barriers)
- [7. Concurrency and reservations](#7-concurrency-and-reservations)
- [8. Memory and upload](#8-memory-and-upload)
- [9. Shaders and pipelines](#9-shaders-and-pipelines)
- [10. Descriptors and bindings](#10-descriptors-and-bindings)
- [11. Swapchain and frames](#11-swapchain-and-frames)
- [12. Client layer](#12-client-layer)
- [13. Diagnostics and failure](#13-diagnostics-and-failure)
- [14. Migration plan](#14-migration-plan)
- [15. What we deliberately do not copy](#15-what-we-deliberately-do-not-copy)
- [16. Where we depart from the study](#16-where-we-depart-from-the-study)
- [17. Open questions](#17-open-questions)
- [Appendix A. Invariants](#appendix-a-invariants)
- [Appendix B. Module map](#appendix-b-module-map)
- [Appendix C. References](#appendix-c-references)

## 1. Thesis

We are building an explicit, Vulkan-1.3-first RHI for a long-running 3D modeling application. The requirements that drive the design are not "support the widest set of hardware" and not "make a legacy call graph fast". They are predictable frame time, safe teardown after hours of editing, clear ownership, and a small number of abstractions that a small team can keep in its head.

Blender is useful because it shows what explicit resource descriptions, delayed destruction, and shader reflection look like in a real DCC application. It is a warning because its API is a GL-shaped surface that a Vulkan backend has to reconstruct into explicit commands and barriers, using a render graph whose scheduler currently does little more than preserve insertion order. We do not need that reconstruction layer, because we never had an OpenGL backend to preserve.

Our design bets are therefore:

- **Explicit command stream.** Commands are recorded into an ordered stream. There is no active shader, active framebuffer, global GPU state, immediate mode, or `Batch` object.
- **Intent-based synchronization.** Callers declare how a resource will be used; the planner derives barriers, layouts, and cross-queue ownership from committed state. No Vulkan layout type appears in the public API.
- **Device-owned lifetime.** A `Device` owns the backend, queues, trackers, retire queue, and caches. The RHI has no process-global GPU state. Long-lived application globals belong to the application layer.
- **Owned command buffers.** Recording produces a value that owns its native command buffer. The recording thread does not hold a queue-wide lease, and submission takes only the short submit lock.
- **Reflection is authoritative.** The shader is the source of truth for bindings, push constants, vertex inputs, fragment outputs, and specialization constants. We do not maintain a parallel hand-written create-info structure.
- **No hidden submission, no hidden wait.** `build()` does not submit. `drop()` does not submit. Present does not submit on behalf of the caller. Teardown is the only place a full-device wait is allowed.
- **No native types in public API.** No `VkImageLayout`, no `VkSemaphore`, no `VkDescriptorSetLayout`, no ash handles. The Vulkan module is private.

A frame that satisfies this design looks like this:

1. The app creates a `Device` from a `DeviceDesc` and receives a `DeviceSelection` naming the adapter, driver, and skipped candidates.
2. Shaders are compiled into `ShaderModule`s with generated `ShaderInterface`s.
3. `DescriptorSetLayout` and `PipelineLayout` are interned; `Pipeline`s are created from complete `PipelineDesc`s with dynamic state and cached by a field-based key.
4. Per-frame data goes into a frame allocator; textures upload through a staging ring; descriptor sets come from a per-frame pool.
5. A `Recorder` on the render thread records explicit passes, draws, and copies; `build()` plans barriers; `submit()` returns a `SyncPoint`; nothing submits behind the caller's back.
6. The swapchain acquires with an opaque token, renders into an offscreen target, and presents with a render-finished token; two or three frames are in flight.
7. Debug labels name every pass and resource; validation and synchronization validation are clean in debug; teardown never panics and never performs a full-device wait during normal operation.

If a proposed change makes any of those steps require a global, a thread-local, a hidden wait, or a Vulkan type in the public API, it is the wrong change.

### 1.1 The constraint we do not inherit

Blender's GPU API is an OpenGL API that grew a Vulkan backend. Many of its mechanisms are compensations for that history, not requirements of an explicit API. The table below is the reason this document does not copy them.

| Blender constraint | Consequence in Blender | Our freedom | Our choice |
|---|---|---|---|
| GL state machine with an active shader and framebuffer | A render graph reconstructs explicit commands from a GL-shaped stream | No legacy API to preserve | Explicit command stream; no active context |
| Binding by texture unit or name | Dense declaration-order descriptor bindings and a `+512` namespace | Reflection can expose explicit `(set, binding)` | Explicit sets and bindings from Slang |
| One descriptor set for the GL uniform model | Whole-set rewrites per draw; `Frequency` never became real sets | No GL uniform grouping | Multiple sets by update frequency |
| Little dynamic state for GL parity | PSO per viewport size and scissor rectangle | Vulkan 1.3 core dynamic state | Viewport, scissor, and most raster state are dynamic |
| CPU-synchronized present with one frame in flight | Full-frame waits and zero wait semaphores | Modern swapchain and timeline synchronization | Semaphore acquire/present, 2-3 frames in flight |
| Compatibility with old drivers and a dummy backend | Feature gaps hidden behind a fallback | We can require a modern baseline | Vulkan 1.3 hard requirement, no dummy backend |
| C++ raw handles and inconsistent ownership | `reinterpret_cast`, non-atomic refcounts, panicking teardown | Rust ownership and typestate | Unique/shared handles, retire queue, non-panicking `Drop` |
| Render graph as a compatibility layer | A second command representation whose scheduler does little | No GL stream to translate | No render graph in the RHI; an optional client-layer scheduler |

What we do keep from Blender is the part that is independent of OpenGL: explicit resource descriptions, load/store ops, texture views, per-object debug labels, timeline-keyed deferred destruction, a declarative shader information model (generated from reflection rather than hand-written), a complete PSO key, persistent pipeline caches, and GPU-driven visibility/compaction at the client layer.

## 2. Decisions at a glance

| # | Area | Decision | Why | Cost |
|---|---|---|---|---|
| D1 | Root object | `Device` is the root; no RHI globals or thread-local current context | Testability, multi-device, explicit teardown | Breaking API change; the app owns the singleton |
| D2 | Handles | Two categories: unique owning handles and shared immutable handles | Correct lifetime without reference-counting everything | Two handle patterns to learn |
| D3 | Recording | Explicit stream plus owned `CommandBuffer`; no queue-wide recording lease | Parallel recording, no hidden state | More types than a single recorder |
| D4 | Barriers | Intent-based, stage-precise `Access`; layout-free public types; merge pass | Portability and correctness without leaking Vulkan | More access variants and planner work |
| D5 | Concurrency | Shared reads only with identical intent fingerprints; otherwise exclusive; busy error instead of blocking | Prevents stale-state and conflicting-layout barrier planning | Callers may need to retry or order submissions |
| D6 | Lifetime | Device-owned retire queue; explicit GC; no `Arc::strong_count`; `Drop` never panics | Long sessions and safe teardown | Explicit GC call in the frame loop |
| D7 | Memory | VMA plus `MemoryClass`, frame allocator, and staging ring; real upload API | No per-upload VMA allocation and no missing map API | New allocator code |
| D8 | Shaders | Slang reflection is authoritative; generated `ShaderInterface`; no parallel DSL | One source of truth for shader ABI | Reflection gaps must be filled before pipelines |
| D9 | Descriptors | Explicit `(set, binding)`, layout-bound sets, interned layouts, frame-pooled sets | Multi-set designs and no per-draw set allocation | Set lifetime and update rules |
| D10 | Pipelines | Complete field-based key, dynamic state, bounded persistent cache | Fewer PSOs and cross-run reuse | Key design and cache management |
| D11 | Present | Swapchain in the RHI, platform surface outside; opaque acquire/present tokens; 2-3 frames | No CPU-synchronized present | Surface integration in the app |
| D12 | Diagnostics | Real labels, object names, typed errors, timestamps, syncval in CI | Debuggability is a feature, not a cleanup task | Implementation work |
| D13 | Backends | Vulkan first; Metal behind a readiness gate; no GL and no dummy backend | Focus and honest feature reporting | Metal is deferred |

## 3. Object and ownership model

### 3.1 Two kinds of handles

We use two handle patterns, and we choose which one applies based on whether the object is mutable or immutable.

**Unique owning handles** are used for objects with identity, state, or a single owner: `Buffer`, `Image`, `ImageView`, `DescriptorSet`, `Swapchain`, `QueryPool`. They are not `Clone`. Dropping the public handle retires the object exactly once. `DescriptorSet` is a special case: the handle is unique from the caller's perspective, but its native set is owned by a descriptor pool; see §10.2.

**Shared immutable handles** are used for objects that are immutable and useful to share: `Pipeline`, `Sampler`, `ShaderModule`, `DescriptorSetLayout`, `PipelineLayout`. They are `Clone` and `Arc`-backed. The last clone retires the object. This is what makes a pipeline cache returnable without giving the caller a borrowed reference and without destroying a pipeline that the cache still holds.

This split is a deliberate departure from the study's blanket "keep owning, non-Clone wrappers". A non-Clone `Pipeline` cannot be handed out by a cache safely: either the cache owns it and the caller cannot keep it, or the caller owns it and the cache cannot retain it. Pipelines and layouts are immutable, so shared handles are the simpler correct model.

### 3.2 Ownership graph

```text
Device (unique root handle)
├── Arc<DeviceInner>        instance, logical device, queues, allocator
├── Arc<RetireQueue>        objects whose public handles were dropped
├── PipelineCache / trackers
└── Arc<FrameResources>     frame allocators and descriptor pools

Resource (unique handle)
├── Option<Arc<dyn RhiBuffer/Image/...>>
├── Weak<RetireQueue>       where to retire; may be gone at teardown
└── Arc<DeviceInner>        via the backend object, keeps native device alive

Shared handle (Pipeline, Sampler, layout, module)
└── Arc<SharedInner>        Drop of the last clone retires
```

The important edge is that `DeviceInner` does **not** own `RetireQueue`, caches, or frame resources. If it did, the graph would contain a cycle (`DeviceInner` → retire queue → object → `DeviceInner`, or `DeviceInner` → cache → pipeline → `DeviceInner`) and the device would never drop. The public `Device` handle owns the retire queue, caches, and frame resources; resources only hold a `Weak` reference to the retire queue and an `Arc<DeviceInner>` for native device state. A resource that outlives the device handle still holds `Arc<DeviceInner>`, so the native device stays alive; its `Drop` sees the weak retire queue fail and destroys its native object directly, which is safe because teardown waited for idle.

This gives us three properties:

- `Drop` never panics because it cannot require a live global manager.
- The device cannot be destroyed underneath a live resource.
- Leaking the last `Device` handle leaks native memory but does not cause a use-after-free or a panic. The application still gets a clear contract: call `Device::shutdown` for an explicit, error-reporting teardown.

### 3.3 Retire queue and lifetime entries

```rust
// src/rhi/lifetime.rs
struct LifetimeEntry {
    retired: bool,
    readers: u32,                      // shared readers
    writer: bool,                      // one exclusive writer
    last_use: HashMap<QueueId, SyncPoint>,
}

struct RetireQueue {
    pending: Mutex<Vec<DiscardedObject>>,
}

impl RetireQueue {
    fn retire(&self, object: DiscardedObject);              // called from Drop; never panics
    fn flush(&self, completed: impl Fn(SyncPoint) -> bool) -> RhiResult<GcStats>;
    fn destroy_all(&self) -> RhiResult<()>;                 // teardown only, after idle
}
```

We reject `Arc::strong_count(object) == 1` as the *user-drop signal*. "Unique" is a proxy for "the user dropped it", not a safety property. Explicit retirement is the real event, and the retire queue already owns an `Arc`; a live user handle should never be inferred from a count.

We still need a check for internal aliases: an `ImageView` holds its `Image`, and a `DescriptorSet` holds every resource it writes. The retire queue must not destroy a native object while one of those internal references is alive. The first implementation uses `Arc::strong_count == 1` for that check, but it is an internal-alias safety check, not the user-drop signal. A later version can replace it with explicit internal-owner counts if profiling or correctness requires it.

`RetireQueue::flush` destroys an object only when it is retired, has no outstanding reservation, no internal alias remains, and every per-queue last-use point has completed. `Device::collect_garbage()` runs this once per frame and reports counts. It is never hidden inside a frame guard; the application decides when to pay for it.

### 3.4 Teardown

`Device::shutdown(self)` is the explicit teardown path. It:

1. Marks the device as shutting down so new resource creation and submission fail.
2. Waits for the device to become idle.
3. Drains the retire queue with `destroy_all`.
4. Drops caches, frame resources, and its queue handles, then releases its `Arc<DeviceInner>`.

The native device is destroyed when the last resource or queue releases its `Arc<DeviceInner>`. If a resource outlives the device handle, its native object is destroyed after the idle wait, and the final `Arc<DeviceInner>` drop performs the native device destruction. `Drop for Device` calls the same path and logs any error instead of panicking. `Drop for a resource` never blocks and never waits for idle; it retires or destroys its native object, and the device's idle wait at teardown is what makes the fallback safe.

## 4. Device and backend selection

### 4.1 The root API

```rust
pub struct DeviceDesc {
    pub app_name: String,
    pub validation: bool,
    pub features: Features,        // required API level, dynamic rendering, sync2, timeline, extended dynamic state; optional BDA
    pub policy: DevicePolicy,      // allow/deny by vendor/device/driver; app-owned
}

pub struct DeviceSelection {
    pub backend: BackendType,
    pub device_name: String,
    pub api_version: u32,
    pub driver: String,
    pub enabled_layers: Vec<String>,
    pub skipped: Vec<SkippedDevice>, // device + reason; never a bare bool
}

pub struct Capabilities {
    pub limits: Limits,
    pub features: FeatureSet,
    pub workarounds: WorkaroundFlags,
    pub pipeline_cache_uuid: [u8; 16],
}

impl Device {
    pub fn new(desc: DeviceDesc) -> RhiResult<Self>;
    pub fn selection(&self) -> &DeviceSelection;
    pub fn capabilities(&self) -> &Capabilities;
    pub fn queue(&self, kind: QueueKind) -> RhiResult<Queue>;
    pub fn create_buffer(&self, desc: BufferDesc) -> RhiResult<Buffer>;
    pub fn create_image(&self, desc: ImageDesc) -> RhiResult<Image>;
    pub fn create_sampler(&self, desc: SamplerDesc) -> RhiResult<Sampler>;
    pub fn create_image_view(&self, image: &Image, desc: ImageViewDesc) -> RhiResult<ImageView>;
    pub fn create_shader(&self, module: ShaderModuleDesc) -> RhiResult<ShaderModule>;
    pub fn create_pipeline(&self, desc: PipelineDesc) -> RhiResult<Pipeline>;
    pub fn create_descriptor_set(&self, layout: &DescriptorSetLayout) -> RhiResult<DescriptorSet>;
    pub fn create_swapchain(&self, surface: Surface, desc: SwapchainDesc) -> RhiResult<Swapchain>;
    pub fn upload_buffer(&self, dst: &Buffer, offset: u64, data: &[u8]) -> RhiResult<()>;
    pub fn collect_garbage(&self) -> RhiResult<GcStats>;
    pub fn wait(&self, point: SyncPoint, timeout: Option<Duration>) -> RhiResult<()>;
    pub fn wait_idle(&self) -> RhiResult<()>;
    pub fn shutdown(self) -> RhiResult<()>;
}
```

`Buffer::new`, `Image::new`, `Sampler::new`, `ImageView::new`, and `Pipeline::new` disappear. Creation belongs to the device, and the device is the only thing that can supply the right allocator, queue registry, and retire queue.

### 4.2 No global backend

The current `StaticSlot` globals in [backend.rs](../../src/rhi/backend.rs), [lifetime.rs](../../src/rhi/lifetime.rs), and [resource.rs](../../src/rhi/resource.rs) are replaced by device-owned state. The `bootstrap()`/`Backend::global()` pair may remain temporarily as an application-layer shim, but it is not part of the RHI's design. The RHI exposes `Device::new`; the application decides whether it wants a `OnceLock<Device>` or an `Arc<Device>`.

This is more than cleanliness. A global backend makes tests order-dependent, makes multi-device impossible, makes teardown order a correctness requirement, and makes `Drop` depend on initialization state. All four costs are avoidable.

### 4.3 Feature and device policy

We keep the hard requirement of Vulkan 1.3 + dynamic rendering + synchronization2 + timeline semaphores + the dynamic-state features promoted from `VK_EXT_extended_dynamic_state` and `VK_EXT_extended_dynamic_state2`. Those are not optional paths with fallbacks; without them the backend is not the backend we designed.

`Features` is a descriptor of what the application requires, and `DeviceSelection` reports why each candidate was rejected. Driver allow/deny lists are application policy, not RHI policy; `DevicePolicy` keeps them out of the backend. We do not add a dummy backend. Trait-level mocks already let us test the RHI without a GPU, and a dummy backend would hide feature gaps rather than expose them.

`Capabilities` is immutable data reachable from `Device`: limits, feature booleans, workarounds, and the pipeline cache UUID. There is no process-global mutable `GCaps` equivalent.

### 4.4 Queues

`QueueKind` starts with `Graphics` and `Transfer`. Async compute is a real modeling-workload need, but it is not part of the first vertical slice; add `Compute` only when the renderer has a workload that benefits from it and the device reports a suitable queue family. A queue is a `Clone` handle over a device-owned queue object; all queues in a device share the submit gate when they map to the same native queue.

## 5. Command recording and submission

### 5.1 No queue-wide recording lease

The current `Queue::recorder()` takes a `Mutex<()>` lease that is held until submit or drop, so only one thread can record to a queue at a time ([queue.rs](../../src/rhi/queue.rs)). That is the wrong default for an editor that wants to record a viewport, a shadow pass, and an upload in parallel.

We remove the recording lease. A `Recorder` borrows the queue for allocation, records into its own command buffer, and returns an owned `CommandBuffer` from `build()`. Recording on different command buffers is independent; the queue's command pool is only touched for allocate/reset/free, and those operations are covered by a short pool lock. Submission takes a separate `submit_gate`, which is shared only when two queue handles map to the same native queue.

The Vulkan rule is on our side: a command pool must not be used concurrently for allocation/reset/free, but command buffers allocated from it can be recorded independently. We do not need a thread-local current context, and we do not need one command pool per thread. If profiling shows pool allocation contention, a per-thread free-list cache is an optimization, not an API change.

### 5.2 Recorder and command buffer

```rust
// src/rhi/queue.rs
pub struct Queue { /* Arc<QueueInner> */ }

impl Queue {
    pub fn recorder(&self) -> RhiResult<Recorder<'_>>; // allocates from the queue pool
    pub fn submit(&self, cb: CommandBuffer, waits: &[SyncPoint]) -> RhiResult<SyncPoint>;
    pub fn submit_for_present(
        &self,
        cb: CommandBuffer,
        acquired: &AcquiredImage,
        present: PresentToken,
    ) -> RhiResult<()>;
    pub fn force_flush(&self) -> RhiResult<()>; // queue idle, never device idle
}

pub struct Recorder<'q> {
    queue: &'q Queue,
    commands: Vec<Command<'q>>,
}

impl<'q> Recorder<'q> {
    pub fn begin_rendering(&mut self, desc: RenderingDesc<'q>) -> RenderPassEncoder<'_, 'q>;
    pub fn begin_compute(&mut self) -> ComputePassEncoder<'_, 'q>;
    pub fn copy_buffer(&mut self, src: &'q Buffer, src_offset: u64, dst: &'q Buffer, dst_offset: u64, size: u64);
    pub fn wait_on(&mut self, point: SyncPoint);
    pub fn build(self) -> RhiResult<CommandBuffer>;
}

pub struct CommandBuffer {
    inner: Option<Arc<dyn RhiCommandBuffer>>,
    origin: QueueId,
    waits: Vec<SyncPoint>,
    transaction: Option<TrackerTransaction>,
    lifetime: Option<LifetimeReservation>,
}
```

`CommandBuffer` owns its native command buffer, carries the state transaction and lifetime reservation, and can be sent to another thread when the backend handle is `Send`. It is only submittable to its origin queue, which is checked before the native submit.

Pass scoping is a type, not an assertion. `RenderPassEncoder` exposes only rendering commands; `ComputePassEncoder` exposes only dispatch commands. The existing planner checks (draw outside a pass, dispatch inside a pass, nested passes, unbalanced labels) remain as defensive checks, but the normal API makes those states unrepresentable.

### 5.3 Build and submit

`build()` is eager and has four ordered stages:

1. **Validate.** Queue capabilities, foreign handles, usage flags, range bounds, pass structure, descriptor compatibility.
2. **Reserve.** Take lifetime and state reservations for every object referenced by the stream. If a conflicting reservation exists, return `ResourceBusy` before any native recording starts.
3. **Plan.** Analyze uses against committed state, produce transitions and cross-queue waits, and create a `TrackerTransaction`.
4. **Record.** Begin the native command buffer, record planned barriers and commands, finish it, and return the owned `CommandBuffer`.

`submit()` is separate:

1. Validate the command buffer's origin queue and any explicit waits.
2. Take the submit gate, allocate the next timeline value, and submit with waits.
3. On success, commit the tracker transaction and lifetime reservation with the returned `SyncPoint`.
4. On failure, recycle the command buffer and roll back both reservations.

Eager recording makes the `Command<'q>` borrows safe: they only need to live until `build()` returns, and they do not leak into the submitted artifact. This is the half of the study's `Send` discussion that matters.

### 5.4 Rollback and failure

A validation failure never begins native recording. A recording failure returns the command buffer to the pool without entering the pending list. A submit failure rolls back the tracker transaction and lifetime reservation, returns the command buffer to the pool, and does not advance the timeline value that other submissions can observe. A dropped `Recorder` or `CommandBuffer` does the same rollback and recycling without panicking.

Commit after a successful submit is a cleanup path, not a fallible user operation. If a tracker lock is poisoned, commit recovers the inner value instead of panicking; the GPU work is already scheduled, and the tracker must not diverge from it. The same rule applies to lifetime last-use commits and to teardown.

This keeps two invariants from the current code, and we should keep the tests that prove them:

- No tracker state is visible until a native submit succeeds.
- A failed build or submit leaves the queue usable and the object reservations released.

## 6. Synchronization and barriers

### 6.1 Access is an intent, not a layout

The public model is a closed set of intents. It is richer than the current `ShaderRead`/`ShaderWrite` pair because image layouts differ between sampled reads, storage reads, attachment reads, and present, and because shader-stage precision matters.

| Intent | Applies to | Stage mask (sync2) | Access mask | Image layout |
|---|---|---|---|---|
| `VertexRead` | buffer | `VERTEX_INPUT` | `VERTEX_ATTRIBUTE_READ` | n/a |
| `IndexRead` | buffer | `INDEX_INPUT` | `INDEX_READ` | n/a |
| `IndirectRead` | buffer | `DRAW_INDIRECT` | `INDIRECT_COMMAND_READ` | n/a |
| `UniformRead(stages)` | buffer | `stages` | `UNIFORM_READ` | n/a |
| `StorageRead(stages)` | buffer | `stages` | `SHADER_READ` | n/a |
| `StorageWrite(stages)` | buffer | `stages` | `SHADER_WRITE` | n/a |
| `SampledRead(stages)` | image | `stages` | `SHADER_READ` | `SHADER_READ_ONLY_OPTIMAL` |
| `StorageImageRead(stages)` | image | `stages` | `SHADER_READ` | `GENERAL` |
| `StorageImageWrite(stages)` | image | `stages` | `SHADER_WRITE` | `GENERAL` |
| `TransferRead` | buffer, image | `TRANSFER` | `TRANSFER_READ` | image: `TRANSFER_SRC_OPTIMAL` |
| `TransferWrite` | buffer, image | `TRANSFER` | `TRANSFER_WRITE` | image: `TRANSFER_DST_OPTIMAL` |
| `ColorAttachmentReadWrite` | image | `COLOR_ATTACHMENT_OUTPUT` | `COLOR_ATTACHMENT_READ` \| `COLOR_ATTACHMENT_WRITE` | `COLOR_ATTACHMENT_OPTIMAL` |
| `InputAttachmentRead` | image | `FRAGMENT_SHADER` | `INPUT_ATTACHMENT_READ` | `SHADER_READ_ONLY_OPTIMAL` |
| `DepthStencilReadWrite` | image | `EARLY_FRAGMENT_TESTS` \| `LATE_FRAGMENT_TESTS` | `DEPTH_STENCIL_ATTACHMENT_READ` \| `DEPTH_STENCIL_ATTACHMENT_WRITE` | `DEPTH_STENCIL_ATTACHMENT_OPTIMAL` |
| `DepthStencilSampledRead` | image | `stages` | `SHADER_READ` | `DEPTH_STENCIL_READ_ONLY_OPTIMAL` |
| `Present` | image | `NONE` | `NONE` | `PRESENT_SRC_KHR` |

`Access` carries `ShaderStageFlags` for shader intents. Descriptor reflection supplies the stage flags; if a use cannot be attributed to a stage, the planner falls back to `ALL_GRAPHICS | COMPUTE`, which is conservative but correct. The current `ALL_GRAPHICS | COMPUTE` mapping for every shader read is a performance problem, not a correctness problem; stage precision is the fix.

Host access is not a command-stream intent. `Buffer::write`, `Device::upload_buffer`, and readback APIs handle host visibility and layout outside the planner. `Present` is in the planner because the swapchain image layout transition is a real barrier.

### 6.2 State granularity

Buffers are tracked as a split set of `[offset, size)` ranges with `{ access, queue }`. Images are tracked as sparse `(aspect, mip, layer)` cells; a missing cell means `UNDEFINED`, so the first use of a subresource generates an initial transition. This is already the right model in [resource.rs](../../src/rhi/resource.rs); we keep it.

Usage validation is part of planning: a copy source requires `TRANSFER_SRC`, a vertex bind requires `VERTEX`, an index bind requires `INDEX`, an indirect read requires `INDIRECT`, a sampled read requires `SAMPLED`, a storage access requires `STORAGE`, and an attachment requires the matching attachment usage. The backend object must expose its declared usage, not only remember it at creation. This closes the current gap where `BufferUsage` is checked when a buffer is created but never when it is used.

The public `Transition` type stays layout-free. The Vulkan backend maps an intent to a layout privately. A public API that exposes `VkImageLayout` would leak the thing that makes a Metal backend hard, and it would teach callers to think in Vulkan rather than in dependencies.

### 6.3 Cross-queue ownership

`ReleaseBuffers` and `ReleaseImages` remain explicit commands that preserve their position in the stream. A release marks the ranges as owned by the target queue; the first use on the target queue emits an acquire barrier and adds the source queue's `SyncPoint` to the target submit's waits. The planner rejects a use that crosses queues without a release.

We keep the general release/acquire model rather than adopting the study's proposed "release must be the last command on the source queue" rule. That rule is a simplification, not a Vulkan requirement. If the source wants the resource back, it can acquire it again; the tracker already enforces ownership. The real rule we need is the one the tracker already applies: no access in a queue that does not own the resource.

### 6.4 Barrier merging

The planner starts with correctness: emit a transition for every use that needs one. A post-pass groups transitions by `(ObjectId, range)` and:

- merges overlapping or adjacent ranges with compatible before/after states;
- widens the destination stage/access masks instead of emitting two barriers;
- coalesces contiguous image `(aspect, mip, layer)` cells with identical transitions;
- drops transitions subsumed by a later transition on the same range in the same batch.

This is an optimization, not a correctness fix. It matters because a mipmapped array can otherwise produce dozens of per-cell image barriers. We should implement it after the first frame works, but the planner should produce a flat, mergeable transition list from the start.

### 6.5 Opaque synchronization values

```rust
// src/rhi/sync.rs
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct SyncPoint {
    queue: QueueId,
    value: u64,
}

impl SyncPoint {
    pub const fn value(self) -> u64;
}
```

`SyncPoint` does not carry a native timeline handle. The device's queue registry resolves the queue id to the backend timeline when it needs to wait. This is required for Metal (`MTLSharedEvent`) and removes a public type that encodes a Vulkan handle.

## 7. Concurrency and reservations

This is the section where we deliberately depart from the study's recommendation.

The study suggests replacing the single `reservation: Option<ReservationId>` with `pending: u32` so two threads can record commands touching the same buffer. A pending count is not sufficient, because barrier planning is stateful, and a single reader/writer bit is still too coarse.

There are two failure modes:

1. A pending write and a concurrent read or write both plan from the same committed state. If the second plan submits after the first, its barrier assumes a previous use that is no longer current.
2. Even two readers can disagree on the required state. A sampled image read needs `SHADER_READ_ONLY_OPTIMAL`, while a storage image read needs `GENERAL`. If both plan from the same committed layout and submit in either order, the second barrier assumes a layout that is no longer current.

A write-bit-only model catches the first case but not the second. The reservation must capture the plan's read intent, not just the fact that it reads.

Our first implementation uses per-object shared-read fingerprints:

```rust
pub struct ReadFingerprint { /* canonical set of read Access intents for this object */ }

enum ReservationKind {
    Read(ReadFingerprint), // shareable only with an equal fingerprint
    Write,                 // exclusive
}

struct ObjectReservation {
    shared: Option<ReadFingerprint>,
    shared_count: u32,
    exclusive: bool,
}
```

The rules are:

- A plan summarizes its uses of each state-tracked object. If any use is a write, the reservation is `Write` and exclusive.
- If all uses are reads, the reservation is `Read(fingerprint)`. Any number of plans may share the same fingerprint.
- A new plan with a different fingerprint, or any `Write`, gets `ResourceBusy`; it does not block. The caller decides whether to submit the earlier work, defer, or record on another resource.
- Lifetime-only objects (pipelines, samplers, layouts, shader modules) use an unconditional shared reservation. They are immutable and do not participate in state planning.
- `ReleaseBuffers`/`ReleaseImages` are writes because they change queue ownership.

Two plans with the same read fingerprint produce the same per-range state wherever each plan touches the object. Neither changes the committed state in a way that invalidates the other's plan, so their barriers are interchangeable and their commits are idempotent. A plan with a different fingerprint could change layout or access state, so it is treated as exclusive. This is stricter than a full read-set merge, but it is correct and keeps the first implementation small. Ranges are deliberately not part of the fingerprint: the state tracker already keys buffer ranges and image cells, and delta commits make disjoint reads safe.

The refinement is to merge compatible read sets and ranges, so a sampled read and a storage read can share when their layouts are compatible (for buffers, nearly always; for images, only when the layout is the same). That is an optimization; the fingerprint rule is the safety invariant.

One implementation consequence is that a tracker transaction must be a delta, not a whole-object snapshot. The current transaction stores a complete `TrackedStates` map per object and commits it wholesale. With shared readers on disjoint ranges, the last commit would overwrite the other plan's range. The target transaction stores the exact buffer ranges and image cells it planned and applies only those under the tracker lock. The planning read can still clone the whole object state; the commit cannot.

Reservations are released on commit, rollback, or drop. The state transaction still commits only after a successful native submit. The first implementation is per object; per-range reservations for buffers and per-subresource reservations for images are a later refinement, because the planner already tracks ranges.

## 8. Memory and upload

### 8.1 Memory classes

`CpuAccess` is too narrow. Replace it with an explicit memory class:

```rust
pub enum MemoryClass {
    Device,        // AutoPreferDevice
    HostUpload,    // AutoPreferHost, sequential write, persistently mapped
    HostReadback,  // AutoPreferHost, random access, persistently mapped
}

pub struct BufferDesc {
    pub name: String,
    pub size: u64,
    pub usage: BufferUsage,
    pub memory: MemoryClass,
}
```

`HostUpload` buffers are mapped once at creation and never unmapped; writes flush the allocation when the memory is not coherent. `HostReadback` uses host-cached memory and is invalidated before reads. `Device` buffers are device-local and are not host-mapped.

### 8.2 Upload and readback

The current backend has no way to put bytes into a buffer at all. We add two paths:

```rust
impl Buffer {
    pub fn write(&mut self, offset: u64, data: &[u8]) -> RhiResult<()>; // HostUpload only
    pub fn read(&self, offset: u64, len: usize) -> RhiResult<Vec<u8>>;  // HostReadback only
}

impl Device {
    pub fn upload_buffer(&self, dst: &Buffer, offset: u64, data: &[u8]) -> RhiResult<()>;
}
```

`Buffer::write` is a host memcpy plus a flush. `Device::upload_buffer` records a staging copy; for small updates (at most 64 KiB and a multiple of 4 bytes) it may use `vkCmdUpdateBuffer`, but the recorded copy is the general path. We do not allocate a new VMA buffer per upload.

### 8.3 Staging ring and frame allocator

Two mechanisms cover the common cases:

- **Staging ring:** a small ring of `HostUpload` buffers used for texture and buffer uploads. A block is reused only after the `SyncPoint` that consumed it has completed. This avoids per-upload allocation and discard-pool churn.
- **Frame allocator:** a 4-8 MiB host-visible, persistently mapped buffer per frame in flight, bump-allocated and reset after the frame's `SyncPoint` completes. It carries uniforms, dynamic vertex data, and small push-constant-sized payloads that do not fit in push constants.

```rust
pub struct FrameAllocator { /* mapped ring, per frame index */ }

impl FrameAllocator {
    pub fn alloc(&mut self, size: u64, alignment: u64) -> RhiResult<BufferSlice>;
    pub fn reset(&mut self);
}
```

`FrameIndex` and frames in flight are application policy. The RHI provides the allocator and the completion point; it does not hide a frame loop.

### 8.4 Images

Add the fields that are expensive to retrofit:

```rust
pub struct ImageDesc {
    pub name: String,
    pub format: ImageFormat,
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub mip_levels: u32,
    pub array_layers: u32,
    pub sample_count: u32,        // 1, 2, 4, 8, 16
    pub dimension: ImageDimension, // D1, D2, D3, Cube
    pub usage: ImageUsage,         // add INPUT_ATTACHMENT, DEPTH_STENCIL_ATTACHMENT
}
```

MSAA is not optional for a modeling application; adding `sample_count` later touches every render target, pipeline key, resolve path, and swapchain path. Add it now even if the first frame uses 1 sample. The format table also needs depth-stencil formats and the sRGB variants that the platform reports; the current set is a placeholder. `SamplerDesc` needs anisotropy, compare op, border color, and unnormalized coordinates, and `ImageViewDesc` needs cube, 3D, and array view types. These are small additions now and expensive retrofits later.

## 9. Shaders and pipelines

### 9.1 Reflection is the interface

Slang reflection is authoritative. We generate a frozen `ShaderInterface` for each compiled program:

```rust
pub struct ShaderInterface {
    pub entry_points: Vec<EntryPointInfo>,
    pub bindings: Vec<Binding>,                 // set, binding, type, count, stages, view type
    pub push_constants: Vec<PushConstantRange>,
    pub vertex_inputs: Vec<VertexInput>,
    pub fragment_outputs: Vec<FragmentOutput>,
    pub spec_constants: Vec<SpecConstant>,
    pub ubo_fields: HashMap<String, Vec<Field>>,
}
```

The current reflection covers bindings, push ranges, and UBO fields ([reflect.rs](../../src/rhi/shader/reflect.rs)). It does not cover vertex inputs, fragment outputs, or specialization constants, and it silently drops bindless spaces (`>= 2`). A pipeline cannot be derived from an incomplete interface. Fill these gaps before building the first pipeline, and make a dropped binding an error rather than a silent omission.

We do not add a hand-written create-info structure. The study's criticism of Blender's `ShaderCreateInfo` is correct: it duplicates the shader, uses non-owning string fields, and has a hand-maintained equality operator that omits fields. A builder DSL is justified only for runtime-generated shaders (material/node graphs), and it should generate Slang or a Slang module, not a parallel declaration.

### 9.2 Modules, layouts, and pipelines

```rust
pub struct ShaderModule { /* SPIR-V + ShaderInterface, shared */ }
pub struct DescriptorSetLayout { /* canonical bindings, shared */ }
pub struct PipelineLayout { /* set layouts + push ranges, shared */ }

pub struct PipelineDesc<'a> {
    pub name: &'a str,
    pub modules: &'a [(&'a ShaderModule, &'a str)], // module + entry point
    pub layout: &'a PipelineLayout,
    pub vertex_layout: &'a VertexLayout,
    pub state: &'a GraphicsState,
    pub attachments: &'a AttachmentKey,
    pub spec_constants: &'a [u32],
}

pub struct PipelineKey {
    pub shader: ShaderId,
    pub vertex_layout: VertexLayoutId,
    pub layout: PipelineLayoutId,
    pub attachments: AttachmentKey,
    pub state: GraphicsState,
    pub spec: Box<[u32]>,
}
```

`ShaderId` is a content hash of the source, entry points, defines, target environment, optimization, debug level, compiler version, and `SHADER_ABI_VERSION`. `VertexLayoutId` and `PipelineLayoutId` are interned ids, not raw handles. `PipelineKey` derives `Hash` and `Eq` over every field; no field is compared by a hash alone, and no key contains a native handle.

All floating-point pipeline state is keyed by bit pattern, not by `f32` equality. `LineWidth`, depth bias, and blend constants are small newtypes with stable bit representations.

### 9.3 Dynamic state

We use the dynamic state promoted into Vulkan 1.3 from `VK_EXT_extended_dynamic_state` and `VK_EXT_extended_dynamic_state2`, enabled as required device features, for the state that changes per draw or per viewport:

- viewport, scissor;
- blend constants, stencil reference;
- depth bias, depth test enable, depth write enable, depth compare op;
- cull mode, front face;
- primitive topology (within a topology class), primitive restart;
- stencil ops and masks where the promoted 1.3 dynamic state supports them.

Static pipeline state is what actually affects compilation: shader stages, vertex input layout, attachment formats and sample count, blend attachment state, multisample state, pipeline layout, and specialization constants. The key stores a `PrimitiveTopologyClass` rather than the exact dynamic topology.

This is the single biggest practical lesson from Blender: its Vulkan backend bakes viewport and scissor into the PSO, so every viewport size and scissor rectangle creates a pipeline. We already have `SetViewport`, `SetScissor`, `SetBlendConstants`, and `SetStencilReference` in the command stream ([command.rs](../../src/rhi/command.rs)); we extend that set and remove those values from the pipeline key.

### 9.4 Pipeline cache

```rust
pub struct PipelineCache { /* bounded LRU + persistent VkPipelineCache */ }

impl PipelineCache {
    pub fn get(&self, key: &PipelineKey) -> Option<Pipeline>;
    pub fn get_or_create(&self, desc: &PipelineDesc<'_>) -> RhiResult<Pipeline>;
    pub fn prewarm(&self, keys: &[PipelineKey]);
    pub fn stats(&self) -> CacheStats;
}
```

The cache is bounded and evicts by LRU. An evicted pipeline is retired, not destroyed immediately; the lifetime tracker protects it while a submitted command buffer may still use it. The persistent `VkPipelineCache` is serialized with a header containing magic, app version, build hash, `pipelineCacheUUID`, driver version, and `SHADER_ABI_VERSION`; a mismatch discards the cache.

Pipeline creation starts synchronous and cache-backed. Async creation is a later optimization: worker threads create pipelines with per-thread `VkPipelineCache` objects (the Vulkan object requires external synchronization), and the caches are merged at frame end. The encoder must reject a pipeline that is not ready; an async API should return a pending handle rather than block the render thread.

## 10. Descriptors and bindings

### 10.1 Explicit sets, layout-bound

We keep explicit `(set, binding)` indices from Slang reflection. We do not use Blender's dense declaration-order assignment, its `+512` slot namespace, or its one-set-per-shader model.

```rust
pub struct DescriptorSetLayout { /* canonical bindings, frozen, shared */ }

impl DescriptorSet {
    pub fn write_uniform_buffer(&mut self, binding: u32, buffer: &Buffer, range: Range<u64>) -> RhiResult<()>;
    pub fn write_storage_buffer(&mut self, binding: u32, buffer: &Buffer, range: Range<u64>) -> RhiResult<()>;
    pub fn write_sampled_image(&mut self, binding: u32, view: &ImageView, sampler: &Sampler) -> RhiResult<()>;
    pub fn write_storage_image(&mut self, binding: u32, view: &ImageView) -> RhiResult<()>;
    pub fn write_sampler(&mut self, binding: u32, sampler: &Sampler) -> RhiResult<()>;
    pub(crate) fn written_resources(&self) -> &[WrittenResource];
}
```

A `DescriptorSet` is created from a layout, not from thin air. The layout is interned by its canonical sorted binding list `(set, binding, type, count, stage flags)`. The pipeline layout is interned by its list of set layouts and push ranges. This is what allows two shaders with identical interfaces to share pipelines and layouts.

`WrittenResource` carries the object id, access intent, range, and a strong reference to the underlying resource. The strong reference keeps buffers, images, views, and samplers alive until the set is destroyed; the planner uses the id and intent to add the resource uses to the command stream. Binding a descriptor set is therefore a real resource use, not an opaque handle.

### 10.2 Pools and update rules

Transient sets come from a per-frame descriptor pool. The pool owns the native set; the public handle is a frame-scoped view. Dropping the handle does not free the native set; the pool reset does, after the frame's `SyncPoint` completes. The set's written-resource references live in the pool-owned object, not in the handle, so a dropped handle cannot release a resource that a submitted command buffer still references.

Persistent sets (materials, bindless tables) need stricter rules. The first implementation should either:

- create a new set for a new version and retire the old one after its last use, or
- allow in-place writes only while the set is not in flight, tracked by a reservation counter and checked in debug.

The first is simpler to reason about and is the default for materials until profiling says otherwise. The second is an optimization, not a correctness shortcut.

### 10.3 Binding and set conventions

`BindDescriptorSets` should carry the `PipelineLayout` (or be validated against the currently bound layout), not a `Pipeline`. The native command needs a layout, and decoupling the two lets a compute pass and a graphics pass share sets without pretending a pipeline is the owner of the layout.

Set numbers are an update-frequency convention, not an RHI concept:

- set 0: frame/view data, updated once per frame;
- set 1: material data, updated on material change;
- set 2: per-draw data, only when push constants cannot carry it.

This is what Blender's `Frequency` intended and never implemented. We document it as a convention and use it in the first renderer, but the RHI does not enforce it.

We use separate sampled-image and sampler bindings, matching Slang reflection and Metal. Combined image samplers would add sampler permutations and a Metal translation problem. The choice is encoded in `DescriptorType` so the layout builder is unambiguous.

Bindless (`space >= 2`) is not silently skipped. Until descriptor indexing is implemented, a reflection result with a bindless space must return an explicit `Unsupported` error naming the binding. A layout that is missing a binding is worse than a compile error.

## 11. Swapchain and frames

### 11.1 Layering

The platform owns the window and surface; the RHI owns the swapchain. The app creates a surface from a raw window handle and passes it to `Device::create_swapchain`. The RHI never includes `winit` or a platform window type.

```rust
pub struct SwapchainDesc {
    pub width: u32,
    pub height: u32,
    pub format: ImageFormat,
    pub present_mode: PresentMode,
    pub frames_in_flight: u32,
}

pub struct AcquiredImage {
    pub image: Arc<SwapchainImage>,
    pub index: u32,
    pub frame: FrameIndex,
    pub acquire: AcquireToken,
}

#[derive(Clone, Copy)]
pub struct PresentToken { /* opaque, device-validated */ }

impl Swapchain {
    pub fn acquire(&mut self, timeout: Option<Duration>) -> RhiResult<AcquiredImage>;
    pub fn present_token(&self, frame: FrameIndex) -> PresentToken;
    pub fn present(&mut self, acquired: AcquiredImage, done: PresentToken) -> RhiResult<PresentOutcome>;
    pub fn reconfigure(&mut self, width: u32, height: u32) -> RhiResult<()>;
}

impl Queue {
    pub fn submit_for_present(
        &self,
        cb: CommandBuffer,
        acquired: &AcquiredImage,
        present: PresentToken,
    ) -> RhiResult<()>;
}
```

A frame using this API looks like:

```rust
let acquired = swapchain.acquire(None)?;
let present = swapchain.present_token(acquired.frame);
queue.submit_for_present(cb, &acquired, present)?; // waits on acquire, signals present
let outcome = swapchain.present(acquired, present)?;
```

`AcquireToken` and `PresentToken` are opaque. The backend may use binary semaphores for acquire/present and timeline semaphores for queue-to-queue dependencies; the public API does not expose either. `SwapchainImage` owns its `Image` and `ImageView` wrappers; `AcquiredImage` holds an `Arc` so it can outlive the `&mut Swapchain` borrow without the wrappers being retired on drop. `submit_for_present` does not present; it only waits for the acquire and signals the render-finished token. `Swapchain::present` is the only call that presents.

### 11.2 Frames in flight

The default is 2 or 3 frames in flight. Each frame has:

- a `FrameIndex`;
- a frame allocator;
- a descriptor pool;
- a render-finished token;
- a completion `SyncPoint`.

The app resets per-frame resources only after the frame's completion point. The RHI provides the primitives; it does not hide a frame guard or a submit inside `present`.

### 11.3 Present behavior

Queue acquire and present are semaphore operations, not CPU waits. A full-frame CPU wait is rejected, as is presenting with zero wait semaphores. The swapchain recreates only itself on `OutOfDate` or `Suboptimal`, never the device.

Surface format is chosen by querying and preferring an sRGB format; it is not hard-coded. Present mode defaults to FIFO, with MAILBOX as an opt-in. Image count starts at `minImageCount + 1`, is clamped to `maxImageCount` when nonzero, and is capped at 3 when the minimum allows it.

Rendering an editor region into an offscreen target and then compositing or blitting it into the swapchain image is a client-layer pattern. It is useful for viewports, but it is not part of the swapchain object.

## 12. Client layer

The RHI does not know about scenes, materials, cameras, lights, selection, or editor regions. The client layer lives in `src/render` and owns:

- `frame.rs`: frame index, in-flight points, per-frame allocator and descriptor pool;
- `pass.rs`: pass descriptions and recording into `RenderPassEncoder`/`ComputePassEncoder`;
- `view.rs`: camera matrices, viewport, visibility;
- `cull.rs`: GPU visibility and compaction;
- `geometry.rs`: `Geometry` (vertex/index buffers, primitive, index range), not a `Batch`;
- `material.rs`: pipeline plus descriptor sets and parameter values;
- `draw.rs`: `DrawItem`, `DrawList`, sorting, and state grouping;
- `graph.rs` (optional): a pass graph if the renderer grows beyond a forward pass list.

A draw is a value, not an object:

```rust
pub struct DrawParams<'a> {
    pub pipeline: &'a Pipeline,
    pub vertex_buffers: &'a [VertexBufferBinding<'a>],
    pub index_buffer: Option<IndexBufferBinding<'a>>,
    pub vertex_count: u32,
    pub instance_count: u32,
    pub first_vertex: u32,
    pub first_instance: u32,
}
```

Geometry grouping is a client concern. A geometry object must not remember a shader pointer, and there is no global active shader to bind implicitly.

For large scenes, the client layer should adopt the GPU-driven pattern that Blender gets right: a visibility/compaction compute pass writes an indirect command buffer and a per-instance resource-id buffer; the vertex shader fetches per-object data by `first_instance + instance_id`; the RHI issues `DrawIndexedIndirect` with a `draw_count` range. The RHI does not need a multi-draw abstraction beyond the indirect call with a count. Culling, sorting, and compaction stay in `src/render`.

## 13. Diagnostics and failure

### 13.1 Real labels and object names

Debug labels must not be silent no-ops. The Vulkan device should retain the `VK_EXT_debug_utils` device dispatch and implement `begin_debug_label`, `end_debug_label`, and `insert_debug_label`. `Device::set_object_name` should use `vkSetDebugUtilsObjectNameEXT`, and resources should be named at creation from `BufferDesc.name`, `ImageDesc.name`, and `PipelineDesc.name`.

The command enum and planner already carry labels and validate nesting ([command.rs](../../src/rhi/command.rs), [resource.rs](../../src/rhi/resource.rs)); the backend just needs to record them.

### 13.2 Typed errors

`RhiError::BackendError(String)` and `InvalidCommand(String)` are too lossy at the API boundary. We want structured errors with enough context to fix the call:

```rust
pub enum RhiError {
    UnsupportedBackend,
    DeviceSelection { skipped: Vec<SkippedDevice> },
    Backend { context: &'static str, source: Option<Box<dyn Error + Send + Sync>> },
    InvalidCommand { context: &'static str, detail: String },
    InvalidSyncPoint { context: &'static str },
    ResourceState { context: &'static str, detail: String },
    ResourceBusy { resource: String },
    Shader { diagnostics: Vec<ShaderDiagnostic> },
    Validation(String),
    DeviceLost,
    OutOfMemory,
    Unsupported { feature: &'static str },
}
```

Shader compilation returns `ShaderDiagnostic { severity, file, line, column, message }` rather than a formatted string. Validation-layer messages are routed to a device-owned queue that the app can poll, not only to the logger. The debug callback already logs; the missing piece is making the same information available to the caller.

### 13.3 Profiling and validation

Add a timestamp query pool and a pass-timing readback behind a profiling feature. Blender's commented-out GPU timers are a gap we should not repeat. Keep synchronization validation enabled in debug and run one headless frame with syncval in CI.

Workarounds live in `Capabilities.workarounds`, with a test-only `Features::force_workarounds` flag. A workaround must never silently change engine-visible behavior, as Blender's Workbench render-pass workaround does.

### 13.4 Device loss and unsupported paths

A lost device fails submits and waits with `RhiError::DeviceLost`; the app recreates the device. There is no automatic recovery, and there is no `todo!()` or `unimplemented!()` on a reachable path. An unsupported feature returns `RhiError::Unsupported { feature }` before native recording begins.

## 14. Migration plan

The current code is a working submission and state-tracking core, but it is organized around global state and a queue lease. The migration should be phased so each phase has a testable acceptance criterion.

| Phase | Deliverable | Acceptance |
|---|---|---|
| 0a | `Device` root, `DeviceInner`, device-owned `RetireQueue`, non-panicking `Drop`, explicit `shutdown` | Existing tests pass; teardown does not panic; a resource can outlive the device handle without a use-after-free |
| 0b | Owned `CommandBuffer`, no recording lease, per-queue command pool with short allocation lock | Two threads record different command buffers on the same queue concurrently; submit is serialized only at the submit gate |
| 0c | Shared-read fingerprint reservations; `ResourceBusy`; transactions commit on submit | Plans with the same read fingerprint can overlap; a write or a different read fingerprint cannot; a failed build releases reservations |
| 1a | Complete reflection: vertex inputs, fragment outputs, specialization constants, no silent bindless drop | A `ShaderInterface` can describe a pipeline without hand-written declarations |
| 1b | `ShaderModule`, `DescriptorSetLayout`, `PipelineLayout`, `PipelineDesc`, `PipelineKey`, synchronous cache | A pipeline is created from reflection plus a `GraphicsState`; key equality is field-based |
| 1c | Descriptor set writes and planner introspection | A shader with set 0 and set 1 records and draws without the "introspection" error |
| 1d | Upload path: `Buffer::write`, `Device::upload_buffer`, staging ring, frame allocator | A texture and a UBO can be filled without per-upload VMA allocation |
| 1e | Offscreen vertical slice | A headless test renders a textured, lit triangle to an image and reads it back |
| 2a | Swapchain acquire/present with opaque tokens, 2-3 frames in flight | A window clears to a color and presents without a CPU wait |
| 2b | Real debug labels and object names; typed validation messages | RenderDoc captures show pass and resource names |
| 2c | `sample_count`, input-attachment usage, depth-stencil formats, usage validation | An MSAA render target is expressible and validated |
| 3a | Barrier merge, stage-precise `Access`, opaque `SyncPoint` | Consecutive barriers merge; no Vulkan handle in public types |
| 3b | Bounded persistent pipeline cache, async compile service, pipeline prewarm | Second run reuses PSOs; compile jobs are pollable without a sleep loop |
| 3c | Query pools, pass timings, bindless descriptor indexing, multi-queue/async compute | Profiling and bindless work; cross-queue release/acquire is exercised |
| 4 | Metal backend behind the same traits; D3D12 only if needed | Triangle renders; no Vulkan types in the trait |

The phases are dependency-ordered, not estimates. Phase 0 is the largest API break and should happen before more code depends on the global accessors. Phase 1 is the vertical slice that makes the RHI useful. Phase 2 makes it presentable. Phase 3 is where the performance work belongs, after correctness is visible.

## 15. What we deliberately do not copy

| Blender mechanism | Why we reject it |
|---|---|
| `Batch` as an object with a shader pointer | Geometry and shader selection belong to the client; it recreates the active-shader model |
| Immediate mode (`imm*`) | A second draw path with its own buffering and shader swapping; a UI geometry builder can emit into the explicit stream |
| Global `GPU_state_*` bitfield | Call-order-dependent mutable state; our commands are explicit |
| Active shader / active framebuffer | A thread-local context that every call mutates; illegal states should be unrepresentable |
| Binding by name or texture unit | Explicit `(set, binding)` from reflection; no string or unit lookups at draw time |
| Dense declaration-order bindings | Inserting a resource shifts every later binding and invalidates caches |
| `Frequency` as a fake set split | It was never implemented; we use real set indices and document a frequency convention |
| Hand-written `ShaderCreateInfo` duplicating the shader | Reflection is authoritative; no parallel declaration with non-owning strings |
| No dynamic state, PSO per viewport/scissor | Dynamic state promoted in Vulkan 1.3; viewport and scissor do not belong in the pipeline key |
| Hash-only PSO key equality | Field-based equality over a complete key; no aliasing through a 64-bit hash |
| Raw-handle identity in PSO keys | Interned stable ids for modules, layouts, and vertex layouts |
| Unbounded PSO caches and dead removal paths | Bounded LRU with explicit retirement and a persistent cache header |
| Grow-only descriptor pools | Per-frame pools reset in bulk; no `FREE_DESCRIPTOR_SET_BIT` leak |
| Per-upload VMA allocation and staging through the discard pool | Staging ring plus frame allocator |
| One descriptor set for everything | Multiple sets by update frequency; no whole-set rewrite per draw |
| CPU-synchronized present with zero wait semaphores | Opaque acquire/present tokens, semaphore-based present, 2-3 frames in flight |
| `render_begin`/`render_step`/`render_end` global frame protocol | A backend-specific frame hook; the app owns its frame loop |
| Dummy backend | Hides feature gaps; trait-level mocks are the test tool |
| Driver blacklists inside the RHI | Application policy in `DevicePolicy` |
| Thread-local `Context::get()` and a global draw-loop mutex | Pass `&Device`/`&Queue`; the app owns scheduling |
| `Arc::strong_count` as the user-drop signal and panicking `Drop` | A count cannot tell whether the user dropped the handle; explicit retirement can. Keep the count only as an internal-alias check; `Drop` never panics |

## 16. Where we depart from the study

The study is the input, not the final word. These are the places where this design makes a different call, and why.

| Topic | Study recommendation | Our choice | Reason |
|---|---|---|---|
| Root object | Keep `Backend::global()`/`bootstrap()` as an app shim | Remove RHI globals; `Device` is the root | The shim preserves the exact coupling we are trying to delete |
| Handles | Keep resource wrappers owning and non-Clone | Split unique owning from shared immutable | Pipeline caches and layout interning need shareable handles |
| Concurrent reservations | Replace the single reservation with `pending: u32` | Shared reads only with equal intent fingerprints; writes exclusive | A pending count allows stale-state and incompatible-layout barrier planning |
| Command pools | Per-thread command pools | Per-queue pool with a short allocation lock, optional per-thread cache | Correct without thread-exit cleanup; recording is already parallel |
| Release/acquire | Require release to be the last command on the source queue | Keep the general model; the tracker enforces ownership | The restriction is a simplification, not a Vulkan requirement |
| Sync points | Make `SyncPoint` opaque | Agree, and make acquire/present tokens opaque as well | Present may need binary semaphores; the public API should not care |
| Present API | `present(queue, image, done)` with a `SyncPoint` | `submit_for_present` + `PresentToken` | Preserves the option of binary semaphores and backend-specific present |
| Bindless | Support explicitly or error | Error until descriptor indexing is implemented | A missing binding in the layout is a draw-time failure; fail early |
| Render graph | No render-graph object in the RHI | Agree; a graph is an optional client-layer scheduler | The RHI command stream is already explicit |

## 17. Open questions

These are the decisions that need a second opinion before they become code.

1. **Should `Device` be cloneable?** I propose a unique root handle, with the app using `Arc<Device>` if it needs shared ownership. This keeps `shutdown` meaningful and avoids two owners racing teardown.
2. **When do we add `QueueKind::Compute`?** The first slice does not need async compute. The renderer will, once there is a culling or post-processing workload that overlaps graphics. Add it when there is a measured need, not preemptively.
3. **Bindless strategy:** descriptor indexing arrays, descriptor buffers, or both? The reflection layer must stop dropping `space >= 2` either way; the question is which backend path we commit to for materials.
4. **Client-layer render graph:** a forward pass list is enough for a viewport, shadows, and post. A graph becomes worth it when passes can be reordered or culled by dependencies. We should not let that decision leak into the RHI.
5. **MSAA policy:** 4x default for viewports, 1x for offscreen utility targets, or per-view? The RHI should expose `sample_count`; the policy belongs to `src/render`.
6. **Descriptor set versioning:** new set per material version versus in-place update guarded by in-flight tracking. Start with new sets; optimize only if material churn is a measured problem.
7. **Async pipeline creation:** do we accept first-use hitches behind a synchronous cache for the first milestone, or invest in pending pipeline handles immediately? I recommend synchronous first, async after the vertical slice.
8. **Metal:** the readiness gate in the study is the contract. The open question is whether to use argument buffers for descriptor sets from the start or to keep a separate-sampler translation layer.
9. **D3D12:** not planned. If it becomes a requirement, the descriptor and barrier models are the two areas that would need a second design pass.
10. **Secondary command buffers:** I propose no. Multiple primary command buffers recorded in parallel cover the same use cases without inheriting render-pass state.
11. **Resource aliasing and transient memory:** useful for large scenes, but not before the allocator and lifetime model are stable. The current image-cell state model is compatible with aliasing later.
12. **Frame pacing:** FIFO by default, MAILBOX opt-in, 2-3 frames in flight. Latency policy is an application concern; the RHI exposes the tokens.
13. **Memory budget and residency:** VMA manages allocation, but a DCC app eventually needs a budget and eviction policy. Keep it out of the first design; revisit when textures are streamed.
14. **Public debug identity:** `ObjectId` stays crate-private. If the client needs stable ids for GPU-driven resource tables, it should use a client-layer id, not the RHI's tracker key.

## Appendix A. Invariants

- The RHI has no process-global GPU state and no thread-local current context.
- Every resource is created from a `Device`; there is no `Buffer::new` or `create_*` free function.
- Native types (`VkImageLayout`, `VkSemaphore`, `VkDescriptorSetLayout`, ash handles) never appear in the public API.
- `Drop` never panics and never submits. Teardown is the only place a full-device wait is allowed.
- A `CommandBuffer` owns its native command buffer, is submittable only to its origin queue, and is `Send` when the backend handle is `Send`.
- Recording does not hold a queue-wide lock; submission takes only the submit gate.
- A state plan is never built from a committed state that another plan can change. A plan shares a resource only with plans that have the same read fingerprint; any write is exclusive.
- Tracker state is committed only after a successful native submit; build, submit, and drop failures roll back.
- A tracker transaction commits only the ranges and cells it planned; it never publishes a whole-object snapshot.
- Image ranges are validated for zero counts, overflow, aspect, mip, and layer bounds before they reach a barrier.
- Buffer and image usage flags are validated against the command that uses them.
- An object is destroyed only when it is retired, has no reservation, no internal alias remains, and every per-queue last-use point has completed.
- GC is explicit and called once per frame; it is not hidden in a frame guard.
- Swapchain acquire and present use opaque tokens; present does not wait on the CPU.
- Pipeline keys are field-based and include every non-dynamic state; no key contains a native handle.
- Pipeline caches and descriptor pools are bounded and reset; no structure grows for the process lifetime.
- Descriptor sets are layout-bound, keep their written resources alive, and are not mutated while in flight.
- Shader interfaces are generated from reflection; no binding is silently dropped.
- Every reachable unsupported path returns a typed error; there is no `todo!()` in production code.

## Appendix B. Module map

| Current | Target |
|---|---|
| `src/rhi/backend.rs` | `device.rs` (`Device`, `DeviceDesc`, `DeviceSelection`, `Capabilities`); `backend.rs` becomes a private trait module |
| `src/rhi/queue.rs` | `queue.rs` (`Queue`, `Recorder`, `CommandBuffer`, submit) |
| `src/rhi/command.rs` | `command.rs` (command enum, pass encoders, binding/draw value types) |
| `src/rhi/resource.rs` | `resource.rs` (wrappers) plus `tracking.rs` (`Access`, `Transition`, `ResourceTracker`) |
| `src/rhi/lifetime.rs` | `lifetime.rs` (`RetireQueue`, `LifetimeEntry`, reservations) |
| `src/rhi/descriptor.rs` | `descriptor.rs` (set layouts, pipeline layouts, pools, sets) |
| `src/rhi/pipeline.rs` | `pipeline.rs` (`PipelineDesc`, `GraphicsState`, `PipelineKey`, cache) |
| `src/rhi/shader/*` | `shader/*` (compiler service, reflection, generated interface) |
| `src/rhi/sync.rs` | `sync.rs` (`SyncPoint`, `FrameIndex`) |
| `src/rhi/types.rs` | `types.rs` (`MemoryClass`, descriptors, formats) plus `memory.rs` and `query.rs` |
| `src/rhi/vk/*` | unchanged location, private implementation of the above |
| `src/util/static_slot.rs` | no longer used by the RHI; keep only if an application-level service needs it |

## Appendix C. References

- [BLENDER-DESIGN-LESSONS.md](BLENDER-DESIGN-LESSONS.md): the Blender 4.4 study's decision record and evidence.
- [Blender GPU study](../blender-gpu/README.md): the four source reports and verification notes.
- [src/rhi/queue.rs](../../src/rhi/queue.rs): current recorder, lease, build, and submit implementation.
- [src/rhi/resource.rs](../../src/rhi/resource.rs): current resource tracker, image cells, release/acquire, and planner.
- [src/rhi/lifetime.rs](../../src/rhi/lifetime.rs): current lifetime tracker and discard manager.
- [src/rhi/vk/device.rs](../../src/rhi/vk/device.rs): current device selection, queue creation, and capability checks.
- [src/rhi/vk/command.rs](../../src/rhi/vk/command.rs): current sync2 barrier mapping and rendering commands.
- [src/rhi/shader/reflect.rs](../../src/rhi/shader/reflect.rs): current reflection extraction and its gaps.
