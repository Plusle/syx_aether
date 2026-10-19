# Blender design lessons for syx_aether's RHI

This is the decision document that closes out the Blender 4.4 GPU study. It does not summarise the four source reports; it converts them into choices for `src/rhi`, with an explicit verdict for each topic, the evidence behind it, and the Rust API shape that follows.

| Field | Value |
|---|---|
| Status | Decision record for the RHI design; supersedes the recommendation sections of docs 01-04 |
| Blender baseline | 4.4 (`source/blender/blenkernel/BKE_blender_version.h:24`), checkout at `05377985` |
| Target baseline | Vulkan 1.3 + dynamic rendering + synchronization2 + timeline semaphores (`src/rhi/vk/device.rs:89-113`); Metal next |
| Scope | `src/rhi/**`; client-layer suggestions are marked `src/render/**` and are not RHI requirements |
| Source changes | None. No Blender or syx_aether source file was modified |

Checking Blender source references requires a local Blender source checkout. Paths beginning `source/blender/...` or `intern/ghost/...` are relative to that checkout's root. Paths beginning `src/...` are relative to this workspace's root. Every non-obvious Blender claim is anchored; where a study report disagrees with source, source wins and Appendix A records the correction.

## Contents

- [How to read the verdicts](#how-to-read-the-verdicts)
- [0. Thesis](#0-thesis)
- [1. Verdict at a glance](#1-verdict-at-a-glance)
- [2. Object and handle model](#2-object-and-handle-model)
- [3. Initialization and backend selection](#3-initialization-and-backend-selection)
- [4. Lifetime and deferred destruction](#4-lifetime-and-deferred-destruction)
- [5. Command recording model](#5-command-recording-model)
- [6. Synchronization and barriers](#6-synchronization-and-barriers)
- [7. Memory allocation](#7-memory-allocation)
- [8. Descriptors and bindings](#8-descriptors-and-bindings)
- [9. Shader authoring and compilation](#9-shader-authoring-and-compilation)
- [10. PSO caching](#10-pso-caching)
- [11. Draw submission and multi-draw](#11-draw-submission-and-multi-draw)
- [12. Threading and async compilation](#12-threading-and-async-compilation)
- [13. Debugging and validation](#13-debugging-and-validation)
- [14. Swapchain and present](#14-swapchain-and-present)
- [15. Metal readiness gate](#15-metal-readiness-gate)
- [16. Prioritized roadmap](#16-prioritized-roadmap)
- [17. Do not copy](#17-do-not-copy)
- [Appendix A. Corrections to the study reports](#appendix-a-corrections-to-the-study-reports)
- [Appendix B. What "done" looks like](#appendix-b-what-done-looks-like)

## How to read the verdicts

- **ADOPT** — the mechanism is right for syx_aether as-is; implement the same idea.
- **ADAPT** — the idea is right but Blender's shape is wrong for an explicit, Vulkan-first API; keep the information, change the interface.
- **REJECT** — do not carry this into the new RHI, even temporarily.

A single topic can carry an ADAPT verdict and still contain REJECT sub-items (for example, "ADAPT the declarative shader information model; REJECT the hand-written parallel declaration"). The tag on the heading is the primary decision.

---

## 0. Thesis

Blender's GPU module is evidence for two opposite conclusions at once.

It is evidence **for** explicit resource description and explicit lifetime management. Texture usage flags, load/store ops, texture views, a declarative shader interface, per-object debug labels, timeline-keyed deferred destruction, and a single submission point per frame are all things an explicit API needs, and Blender's Vulkan backend shows what they look like in practice.

It is evidence **against** GL-shaped call ordering. One active shader, one active framebuffer, a mutable global state machine, `Batch` objects that carry a shader pointer, binding by texture unit or by name, immediate mode, and `render_begin`/`render_step`/`render_end` hooks all exist because the callers predate Vulkan. Blender's Vulkan backend pays for them by rebuilding explicit commands and barriers from a GL-shaped stream, using a render graph whose scheduler currently does nothing but select nodes in insertion order (`source/blender/gpu/vulkan/render_graph/vk_scheduler.hh:26-45`).

syx_aether has already made the important choice. `Command` is an explicit stream (`src/rhi/command.rs:107-200`); `RenderingDesc` carries per-attachment load/store (`src/rhi/command.rs:34-58`); `ResourceTracker` plans barriers from declared uses (`src/rhi/resource.rs:606-697`); `SyncPoint` is a queue timeline value (`src/rhi/sync.rs:5-32`); resources have owning wrappers and deferred destruction (`src/rhi/resource.rs:253-260`, `src/rhi/lifetime.rs:121-207`). The architecture is closer to Vulkan than Blender's API is.

The risk is therefore **not** over-abstraction. It is under-implementation plus a handful of shortcuts that will be expensive to unwind once they are load-bearing:

1. pipelines and descriptor sets are stubs (`src/rhi/vk/device.rs:366-372`);
2. the planner refuses non-empty descriptor sets, so any real shader is un-recordable (`src/rhi/resource.rs:1203-1207`);
3. there is no upload path (`src/rhi/vk/resource.rs` has no map/write/staging API) and no swapchain;
4. stage I/O is not reflected, so a pipeline descriptor cannot be derived from a shader (`src/rhi/shader/reflect.rs:295-405`);
5. ownership shortcuts: a recording lease that serializes all recording on a queue (`src/rhi/queue.rs:95-103`), global trackers reached through `StaticSlot`, `Arc::strong_count` as a liveness test (`src/rhi/lifetime.rs:84-93`), `Drop` impls that panic if a global is missing (`src/rhi/resource.rs:253-260`), and debug labels that are silently no-ops (`src/rhi/vk/command.rs:498-502`).

The recommendation is to **finish the vertical slice and fix the ownership shortcuts** (the fifth item), not to redesign the object model. Everything below is in service of that.

## 1. Verdict at a glance

| § | Topic | Verdict | Decision |
|---|---|---|---|
| §2 | Object / handle model | ADAPT | Keep owning handles and internal `ObjectId`; add binding value types; no `Batch` object |
| §3 | Initialization and backend selection | ADAPT | `Device` owns the graph; selector returns diagnostics; no dummy backend; no lazy global init |
| §4 | Lifetime and deferred destruction | ADAPT | Device-owned retire queue keyed on `SyncPoint`; non-panicking `Drop`; concurrent reservations allowed |
| §5 | Command recording model | ADOPT | Explicit stream + plan + eager build; no render-graph object; no recording-time queue lease |
| §6 | Synchronization and barriers | ADAPT | Keep intent-based tracker; make access stage-precise; merge barriers; make `SyncPoint` opaque |
| §7 | Memory allocation | ADAPT | VMA auto + `MemoryClass` + frame-scoped linear allocator; no per-upload VMA allocation |
| §8 | Descriptors and bindings | ADAPT | Explicit `(set, binding)`; cache set layouts; intern pipeline layouts; frame-pooled sets |
| §9 | Shader authoring and compilation | ADAPT | Reflection is authoritative; generate the interface; reject a parallel create-info DSL |
| §10 | PSO caching | ADAPT | Complete key, dynamic state, interned IDs, bounded cache, persistent driver cache, async creation |
| §11 | Draw submission and multi-draw | ADOPT | Explicit draws + indirect ranges; GPU compaction at the client layer; no multi-draw dependency |
| §12 | Threading and async compile | ADAPT | Explicit device/queue ownership; per-thread command pools; dedicated compiler service; no sleep loop |
| §13 | Debugging and validation | ADOPT | Real debug labels, validation in debug, typed diagnostics, timestamp queries |
| §14 | Swapchain and present | ADAPT | `Swapchain` in the RHI with platform-owned surface; semaphore-based present; 2-3 frames in flight |

## 2. Object and handle model

**Blender.** The public API is procedural C++ over a backend-neutral class hierarchy: opaque `struct GPU*` handles and free functions, with implementations recovered by `reinterpret_cast` at the boundary (`source/blender/gpu/intern/gpu_context_private.hh:145-156`). Ownership is inconsistent: only `Texture` and the `VertBuf` *handle* are reference-counted, and neither counter is atomic (`source/blender/gpu/intern/gpu_texture_private.hh:86-88`, `source/blender/gpu/GPU_vertex_buffer.hh:86-88`); framebuffer attachments store raw pointers and do not keep their textures alive (`source/blender/gpu/intern/gpu_framebuffer.cc:70-122`), while `Texture::~Texture()` detaches itself from attached framebuffers (`source/blender/gpu/intern/gpu_texture.cc:42-48`). A `Batch` is a geometry bundle that also stores a shader pointer "only for uniform binding", marked deprecated (`source/blender/gpu/GPU_batch.hh:91-92,269-271`); `GPU_batch_set_shader` binds it immediately (`source/blender/gpu/intern/gpu_batch.cc:226-230`) and `GPU_batch_draw` binds it again before drawing (`source/blender/gpu/intern/gpu_batch.cc:413-423`).

**Tradeoff.** GL needed a VAO cache and a "which shader is active" convention, so a `Batch` that owns buffers and remembers a shader made call sites cheap. The cost is that geometry, shader selection, and draw submission are entangled, and lifetime is split across four ownership styles. For an application with a controlled call graph this is survivable; for a reusable RHI it reproduces the bug classes.

**Recommendation — ADAPT.** Keep the owning-handle discipline that syx_aether already has, and keep `ObjectId` as the internal identity used by the tracker and lifetime systems. Do **not** add a `Batch` object, and do not put geometry grouping in the RHI. What syx_aether lacks is not an object but a set of small value types for "resource + range" that commands can carry.

**Changes in syx_aether.**

- Keep `Buffer`, `Image`, `ImageView`, `Sampler`, `Pipeline`, `DescriptorSet` as owning, non-`Clone` wrappers that retire on `Drop` (`src/rhi/resource.rs:208-402`, `src/rhi/pipeline.rs:8-47`, `src/rhi/descriptor.rs:8-46`).
- Keep `ObjectId` crate-private (`src/rhi/lifetime.rs:23-49`). Do not expose it as the public handle; it is a tracker key, not a stable user-facing identity.
- Add binding value types next to `Command`: `BufferBinding { buffer, offset, size }`, `VertexBufferBinding { buffer, offset }`, `IndexBufferBinding { buffer, offset, ty }`, and `DrawParams { pipeline, vertex_buffers, index_buffer, ... }`. A draw becomes a value, not an object.
- Put `Geometry` (buffers + primitive + index range) in `src/render/`, not `src/rhi/`. It is a client-layer convenience, exactly like Blender's `gpu::Batch` is a GL convenience.
- Do not add reference counting to `Texture`-like objects. `Arc` inside the backend object is enough; public handles stay single-owner and `ImageView` keeps its `Arc<dyn RhiImage>` so a view cannot outlive its image (`src/rhi/vk/resource.rs:339-431`).

```rust
// src/rhi/command.rs — values the command stream carries
#[derive(Clone, Copy)]
pub struct BufferBinding<'a> {
    pub buffer: &'a Buffer,
    pub offset: u64,
    pub size: u64,
}

#[derive(Clone, Copy)]
pub struct VertexBufferBinding<'a> {
    pub buffer: &'a Buffer,
    pub offset: u64,
}

#[derive(Clone, Copy)]
pub struct IndexBufferBinding<'a> {
    pub buffer: &'a Buffer,
    pub offset: u64,
    pub ty: IndexType,
}

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

## 3. Initialization and backend selection

**Blender.** There are two independent initializations. `GPU_init()` sets a static flag and builds the shader dependency graph, the create-info registry, codegen, and batch presets (`source/blender/gpu/intern/gpu_init_exit.cc:21-38`); the backend itself is created lazily on the first `GPU_context_create()`, under a mutex, and destroyed with the last context (`source/blender/gpu/intern/gpu_context.cc:132-164`). Backend selection is a process-global static that defaults to OpenGL (`gpu_context.cc:268`); `GPU_backend_type_selection_detect()` only adds OpenGL (or Metal when OpenGL is not built) unless an override was set (`gpu_context.cc:305-327`), so **Vulkan is opt-in in 4.4**. Support probing creates a throwaway Vulkan instance, enumerates physical devices, and checks a feature/extension list (`source/blender/gpu/vulkan/vk_backend.cc:164-236`). Driver allow/deny policy is duplicated between GHOST and the GPU module (`source/blender/gpu/vulkan/vk_ghost_api.hh:15-28`). `GPU_BACKEND_NONE` exists and instantiates a `DummyBackend` (`gpu_context.cc:387-389`).

**Tradeoff.** Lazy global initialization is convenient for an application with one window system and one device, and the "backend dies with the last context" rule keeps teardown ordering simple. The cost is that "is the GPU initialised?" has two answers, the backend is not testable in isolation, multi-device is not expressible, and selection failure is a boolean rather than a diagnosis.

**Recommendation — ADAPT.** syx_aether's current bootstrap already does the important part correctly: it installs the trackers *before* creating the device and unwinds them on failure (`src/rhi/backend.rs:112-137`), and `terminate()` drains the discard manager before dropping the backend (`src/rhi/backend.rs:151-172`). Keep that as an application convenience, but make a `Device` the primary object and selection a diagnostic-producing operation.

**Changes in syx_aether.**

- Introduce `Device::new(DeviceDesc) -> RhiResult<Device>` owning instance, physical/logical device, allocator, queues, capabilities, retire queue and caches. `Backend::global()` / `bootstrap()` remain as an app-entry shim (`src/rhi/backend.rs:40-140`), not as the constructor that resources reach for.
- Replace the inline feature checks in `Device::new` (`src/rhi/vk/device.rs:89-113`) with a `Features` descriptor and a single validation function. Return a `DeviceSelection` that names the chosen device, API version, driver version, and why each skipped candidate was rejected. Blender's blacklists (`vk_backend.cc:61-90`) are application policy; accept a `DevicePolicy` and never duplicate it.
- Keep the hard requirement of Vulkan 1.3 + dynamic rendering + synchronization2 + timeline semaphores. Do not add a dummy backend: tests already use trait-level mocks (`src/rhi/queue.rs:533-761`), and a dummy backend hides feature gaps instead of exposing them.
- Make `bootstrap(BackendType::Metal)` return `UnsupportedBackend` (it does), but remove the panic path in `ShaderConfig::for_backend` (`src/rhi/shader/compiler.rs:51-53`); an unsupported target is an error, not a crash.
- Put capabilities on `Device` as data: limits, feature booleans, and workaround bits. Blender's `GCaps` is a process-global mutable struct (`source/blender/gpu/intern/gpu_capabilities_private.hh:15-74`); a Rust equivalent should be an immutable `&Capabilities` reachable from `Device`, and a `--debug-gpu-force-workarounds` analogue should be a test-only `Features` flag, as in Blender (`vk_backend.cc:347-367`).

```rust
pub struct DeviceDesc {
    pub app_name: String,
    pub validation: bool,
    pub features: Features,       // required API level, dynamic rendering, sync2, timeline, BDA
    pub policy: DevicePolicy,     // allow/deny by vendor/device/driver, app-owned
}

pub struct DeviceSelection {
    pub backend: BackendType,
    pub device_name: String,
    pub api_version: u32,
    pub driver: String,
    pub skipped: Vec<SkippedDevice>, // device + reason; never a bare bool
}

impl Device {
    pub fn new(desc: DeviceDesc) -> RhiResult<Self>;
    pub fn capabilities(&self) -> &Capabilities;
    pub fn queue(&self, kind: QueueKind) -> RhiResult<Queue>;
}
```

## 4. Lifetime and deferred destruction

**Blender.** Deferred destruction is a timeline-keyed pool. `TimelineResources<Item>` is a vector of `(timeline, item)` ordered by non-decreasing timeline; `remove_old(current, deleter)` destroys the prefix that is provably complete (`source/blender/gpu/vulkan/vk_resource_pool.hh:20-65`). There are two pools: the active context's pool and a device orphan pool for deletions from threads without a context; at submit the context pool is moved into the orphan pool and restamped with the submission timeline (`source/blender/gpu/vulkan/vk_device.cc:394-397`). Destruction happens in `end_frame()` and, in background mode, in `render_end()` (`vk_context.cc:146-150`, `vk_backend.cc:558-573`). Forced teardown passes `UINT64_MAX`; `free_immediately` exists for device teardown (`vk_buffer.cc:189-200`).

**Tradeoff.** This is simple, conservative, and safe: a resource freed during frame N is destroyed after submission N completes, regardless of which pass used it. The granularity is the submission, not the resource's true last use, and the safety depends on every resource kind being in the pool and on callers discarding rather than destroying. Blender also demonstrates the failure mode of a global service: teardown needs a separate immediate-free path because the normal path cannot run.

**Recommendation — ADAPT.** syx_aether's `DiscardManager` + `LifetimeTracker` is the same model and is already keyed on `SyncPoint` (`src/rhi/lifetime.rs:106-207`). Keep it, but move it under `Device`, make `Drop` non-panicking, and stop using `Arc::strong_count` as a liveness test.

**Changes in syx_aether.**

- **Own the retire queue by `Device`.** Each backend object captures an `Arc<RetireQueue>` at creation; `Drop` pushes into it and never calls `backend()` or `DiscardManager::with(...).expect(...)`. Today every wrapper's `Drop` panics if the global manager is missing (`src/rhi/resource.rs:253-260`, `src/rhi/pipeline.rs:41-47`, `src/rhi/descriptor.rs:39-45`), which makes teardown order a correctness requirement. `Device::terminate()` drains the queue after `wait_idle`, before destroying the allocator and device.
- **Replace `Arc::strong_count(object) == 1`** (`src/rhi/lifetime.rs:84-93`) with explicit retirement. The discard queue already holds an `Arc`; "unique" is a proxy for "the user dropped it", not a real safety property. Retire with a final `SyncPoint` and leave ownership to `Arc`.
- **Allow concurrent pending reservations.** `LifetimeTracker::reserve` rejects an object already reserved by another pending submission (`src/rhi/lifetime.rs:248-263`), which makes two threads unable to record commands touching the same buffer. Replace the single `reservation: Option<ReservationId>` with `pending: u32`; increment at build, decrement on commit or rollback, and keep the "retired with a pending build" window closed.
- **Make garbage collection explicit.** `Device::collect_garbage()` returns counts and is called once per frame; `Drop` never blocks. Keep `wait_all()` for teardown. Blender's GC is driven from `end_frame` and a background hook (`vk_context.cc:146-150`, `vk_backend.cc:558-573`); syx_aether should not hide it inside a frame guard either — the app decides when to pay for it.
- **Order related objects.** `VkImageView` owns `Arc<dyn RhiImage>` (`src/rhi/vk/resource.rs:339-431`), so a view keeps its image alive. Retire the view before the image, and assert in debug that no retired object is still referenced by a live object with a longer lifetime.

```rust
// src/rhi/lifetime.rs
struct LifetimeEntry {
    retired: bool,
    pending: u32,                         // number of built-but-not-submitted recorders
    last_use: HashMap<QueueId, SyncPoint>,
}

impl RetireQueue {
    fn retire(&self, object: DiscardedObject);     // called from Drop; never panics
    fn flush(&self, completed: impl Fn(SyncPoint) -> bool) -> RhiResult<GcStats>;
    fn destroy_all(&self) -> RhiResult<()>;        // teardown only, after wait_idle
}
```

## 5. Command recording model

**Blender.** The shared backend interface has no command-buffer concept at all: `GPUBackend` exposes resource allocators, compute dispatch, `context_alloc`, and three frame hooks (`source/blender/gpu/intern/gpu_backend.hh:33-64`). On Vulkan, commands are recorded by a single background worker that pops render graphs from a queue, asks the scheduler for an ordered node list, builds barriers, records into a **primary** command buffer, submits once with a timeline signal, and parks the buffer until that timeline completes (`source/blender/gpu/vulkan/vk_device.cc:432-530`). Metal records immediately into encoders (`source/blender/gpu/metal/mtl_command_buffer.mm:41-179`); OpenGL records into implicit driver state. The Vulkan graph is renewed at every present, and a context switch flushes the graph *without* submitting it (`source/blender/gpu/vulkan/vk_context.cc:136`).

**Tradeoff.** The graph gives automatic barriers and render-scope splitting, at the cost of a second command representation (nodes, links, side storage) and a scheduler that currently does nothing. It also hides submission timing, which is why Blender ends up with an implicit full-frame flush inside the present callback (`vk_context.cc:394-395`). The shared API cannot reuse any of this across backends, so each backend grows its own recording model.

**Recommendation — ADOPT.** syx_aether's explicit stream is the right model. Keep `Command` as a flat enum, keep `Recorder` as a builder, keep `build()` eager, and keep `submit()` returning a `SyncPoint`. REJECT introducing a render-graph object as the recording API; a graph is a *planning* technique (already present as `ResourceTracker::plan`, `src/rhi/resource.rs:606-697`), not a second command representation. Also REJECT holding a queue lease for the duration of recording.

**Changes in syx_aether.**

- **Remove the recording lease.** `Recorder` holds a `MutexGuard` on the queue from `recorder()` until the built recorder is submitted or dropped (`src/rhi/queue.rs:95-103,168-174,452-487`), so only one thread can record to a queue at a time. Blender gets away with a single submission thread; syx_aether wants a render thread plus workers. Command pools are already per-queue (`src/rhi/vk/queue.rs:114-118`); make them per-thread (a small pool cache keyed by thread) and take the submission mutex only inside `submit_buffer`, which already exists (`src/rhi/vk/queue.rs:221-229`).
- **Own the command buffer explicitly.** Replace `BuiltRecorder<'queue>` (which borrows the queue) with an owned `CommandBuffer` produced by `Recorder::build()`. Then `Queue::submit(cb, &[SyncPoint])` can be called from a different thread, and `CommandBuffer` can implement `Send` when the backend handle is `Send`. Today eager recording makes the borrow-only `Command` harmless; owning the *result* is the missing half.
- **Make pass scoping a type, not an assert.** The planner already rejects draws outside a render pass, dispatch inside one, nested passes, and unbalanced labels (`src/rhi/resource.rs:1071-1123,1227-1236`). Keep these checks, but also expose `RenderPassEncoder<'pass>` (only `draw*`/bindings) and `ComputePassEncoder<'pass>` (only `dispatch*`). Blender relies on `BLI_assert`s in `Manager::generate_commands` (`source/blender/draw/intern/draw_manager.cc:207-212`); Rust can make the illegal state unrepresentable.
- **Split build into validate / plan / record** internally so a failed validation never begins recording. The existing test `validation_failure_does_not_begin_recording_or_poison_the_lease` (`src/rhi/queue.rs:877-895`) is the right invariant; keep it after the lease is removed.
- **No hidden submission points.** `Drop` of a built recorder must not submit; `present` must not submit on behalf of the caller; frame guards must not flush. Blender violates all three (present flushes with `WAIT_FOR_COMPLETION`, `vk_context.cc:394-395`).

```rust
// Explicit, owned command buffer; recording is lock-free per thread
let queue = device.queue(QueueKind::Graphics)?;
let mut rec = queue.begin()?;                 // per-thread command pool, no queue-wide lease
{
    let mut pass = rec.begin_rendering(&rendering_desc);
    pass.bind_pipeline(&pipeline);
    pass.draw(&DrawParams { /* ... */ });
}                                             // pass ends, render pass is closed
let cb = rec.build()?;                        // validates, plans barriers, records natively
let point = queue.submit(cb, &[upload_done])?; // returns SyncPoint; no hidden wait
```

## 6. Synchronization and barriers

**Blender.** The public synchronization model is a coarse enum of future uses: "can now be sampled", "can now be used as an indirect buffer", and so on (`source/blender/gpu/GPU_state.hh:29-61`). The Vulkan backend turns this into layout transitions and access barriers (`vk_command_builder.cc:399-410`), derives them from per-resource modification stamps and accumulated access/layout state (`render_graph/vk_resource_state_tracker.hh:8-21,75-90,113-160`), merges image barriers per image (`vk_command_builder.cc:725-763`), and uses `TOP_OF_PIPE` when no prior use is known because it targets Vulkan 1.2 (`vk_command_builder.cc:431-438`). Barriers inside a dynamic-rendering scope are forced to framebuffer-space stages (`:442-451`), and all barriers carry `BY_REGION` (`:458-466`). Metal translates the same access classes into `MTLBarrierScope` or encoder breaks (`source/blender/gpu/metal/mtl_command_buffer.mm:581-661`); OpenGL maps them to `glMemoryBarrier`.

**Tradeoff.** A coarse enum is portable and easy for callers, but it under-specifies the subresource, the stage, and the intended layout; the backend compensates with a tracker and guesses. That works because the tracker is the real model, but it means the public API teaches callers to think in hazards rather than in dependencies, and it cannot express cross-queue ownership or readback intent precisely.

**Recommendation — ADAPT.** syx_aether's model is already better than Blender's public one: `Access` is a closed set of intents, state is per buffer range and per image subresource, and `Transition` is explicit (`src/rhi/resource.rs:404-439,467-512`). Keep that. Adapt three things: make access stage-precise, merge barriers, and stop leaking the timeline semaphore handle through `SyncPoint`.

**Changes in syx_aether.**

- **Stage-precise reads and writes.** `Access::ShaderRead` currently maps to `ALL_GRAPHICS | COMPUTE_SHADER` (`src/rhi/vk/command.rs:572-575`). Blender's per-binding access masks are more precise (`vk_shader_interface.cc:302-339`). Change `ShaderRead`/`ShaderWrite` to carry `ShaderStageFlags` (or split into vertex/fragment/compute variants) and map to the corresponding stage. This is the difference between a barrier that serializes all graphics work and one that does not.
- **Add the missing intents:** `HostRead`, `HostWrite`, `Present`, `ColorAttachmentRead` (input attachment), and a depth-only read. Use `Present` to stop hand-writing layout changes in the swapchain path, and `HostRead` to enforce that readback was declared at creation, as Blender does with `GPU_TEXTURE_USAGE_HOST_READ` (`gpu_texture.cc:531-538`).
- **Merge barrier batches.** `plan` emits one `Transition` per use and `VkCommandBuffer::barrier` builds one `vkCmdPipelineBarrier2` per planned batch (`src/rhi/vk/command.rs:81-161`). Add a merge pass keyed by `(ObjectId, range)` that widens destination masks and drops subsumed barriers, mirroring `vk_command_builder.cc:725-763`. This is an optimization, not a correctness fix; do it after the first frame works.
- **Make `SyncPoint` opaque.** It currently stores the raw `VkSemaphore` as `timeline` (`src/rhi/vk/queue.rs:231`) and reconstructs the semaphore with `vk::Semaphore::from_raw` in the device and queue (`src/rhi/vk/device.rs:421`, `src/rhi/vk/queue.rs:206`). Keep `SyncPoint { queue: QueueId, value: u64 }` and resolve the semaphore through the device's queue registry. This is required for Metal (`MTLSharedEvent`) and removes a public type that encodes a Vulkan handle.
- **Keep the ownership release/acquire model.** `ReleaseBuffers`/`ReleaseImages` plus implicit acquire detection is better than Blender, which has one queue and no ownership transfers (`src/rhi/resource.rs:650-677,721-786`; Blender's single queue is `vk_device.hh:146-154`). Add one rule: a release must be the last command on the source queue, and the destination's submit must wait on the release submission's `SyncPoint`.
- **Keep the layout-free public type.** `Transition` has no `VkImageLayout`; `map_image_layout` is private (`src/rhi/vk/command.rs:593-603`). Blender leaks layouts into graph nodes and pays for it in descriptor binding (`vk_descriptor_set.cc:149-207`); do not do that.
- **Enable and use syncval.** Validation is already on in debug and synchronization validation is explicitly enabled (`src/rhi/vk/instance.rs:43-70`). Add a debug-only `Device::verify_sync()` helper that waits and checks completion, and make CI run one headless frame with syncval.

## 7. Memory allocation

**Blender.** VMA is created with `VK_API_VERSION_1_2`, no custom pools, no flags, and every allocation uses `VMA_MEMORY_USAGE_AUTO` (`source/blender/gpu/vulkan/vk_device.cc:206-214`). The recipes are conventional: device-local preferred for resources, host-visible + sequential write for uploads, host-cached + random for readback, optimal tiling for all textures (`vk_buffer.cc:57-62`, `vk_texture.cc:541-575`, `vk_staging_buffer.cc:17-32`, `vk_texture.cc:193-203`). Host-visible allocations are mapped once at creation and never unmapped, with an explicit `vmaFlushAllocation` for non-coherent memory (`vk_buffer.cc:74-76,96-101`). Small updates use `vkCmdUpdateBuffer` under an assert of `size <= 65536 && size % 4 == 0` (`vk_buffer.cc:86-94`); zero-size buffers are rounded to 1 byte (`vk_buffer.cc:44-49`). There is no transient/linear allocator: every upload allocates a staging buffer and throws it into the discard pool (`vk_staging_buffer.cc:14-33`).

**Tradeoff.** VMA auto plus persistent mapping is the right default and keeps the backend small. Per-upload VMA allocations are cheap only because VMA suballocates; they still add allocation, map, discard, and free-list churn to every upload, and they make upload latency depend on the allocator. The missing frame allocator is why Blender's immediate mode needs its own 4 MiB mapped buffer (`vk_immediate.cc:139-175`) — a special case of a general mechanism.

**Recommendation — ADAPT.** Keep VMA and `AUTO`, keep persistent mapping, and add the two things Blender lacks: an explicit memory class in the public descriptor, and a frame-scoped linear allocator for staging, uniforms, and dynamic vertex data. Do not copy per-upload allocation.

**Changes in syx_aether.**

- Add `MemoryClass { Device, HostUpload, HostReadback }` to `BufferDesc` (alongside or replacing `cpu_access`, `src/rhi/types.rs:25-38`). `HostUpload` maps to `AutoPreferHost` + sequential write + persistent mapping; `HostReadback` to host-cached + random; `Device` to `AutoPreferDevice`. The current mapping (`src/rhi/vk/resource.rs:59-77`) is the right start but cannot express "host-visible and device local" or "mapped for reads".
- Add the upload API that does not exist at all: `Buffer::write(&mut self, offset, &[u8])` for `HostUpload` (memcpy + flush) and `Device::upload_buffer(&Buffer, offset, &[u8])` recording a staging copy. For small sizes, use `vkCmdUpdateBuffer` with a documented 64 KiB / 4-byte alignment limit — Blender discovered the constraint the hard way (`vk_buffer.cc:88`). Prefer the recorded copy for anything larger.
- Add a `FrameAllocator`: a 4-8 MiB host-visible, persistently mapped buffer per frame in flight, bump allocated and reset after the frame's `SyncPoint` completes. This is Blender's immediate buffer idea made general (`vk_immediate.cc:139-175`) and Metal's scratch ring (`mtl_memory.hh:469-524`); both backends independently converged on it. Use it for uniforms, dynamic vertices, and small staging copies.
- Replace per-upload staging allocation with a small ring of `HostUpload` buffers. Blender's "allocate + discard per upload" (`vk_staging_buffer.cc:14-33`) is exactly what not to copy.
- Add `sample_count` to `ImageDesc` now. syx hard-codes `TYPE_1` (`src/rhi/vk/resource.rs:218`) and Blender hard-codes 1 in the pipeline (`vk_pipeline_pool.cc:111`), which is one of the reasons its PSO key is under-specified. Retrofitting MSAA later touches every render target, pipeline key, and resolve path.
- Keep `check_image_support` (`src/rhi/vk/device.rs:198-225`) and optimal tiling only. Add linear tiling only if a real readback/staging need appears.

```rust
pub enum MemoryClass { Device, HostUpload, HostReadback }

impl Buffer {
    pub fn write(&mut self, offset: u64, data: &[u8]) -> RhiResult<()>; // HostUpload only
}
impl Device {
    pub fn upload_buffer(&self, dst: &Buffer, offset: u64, data: &[u8]) -> RhiResult<()>; // recorded copy
    pub fn frame_allocator(&self, frame: FrameIndex) -> &FrameAllocator;
}
```

## 8. Descriptors and bindings

**Blender.** Shaders declare resources by kind, slot, and name, grouped by `Frequency::{PASS, BATCH, GEOMETRY}` (`source/blender/gpu/intern/gpu_shader_create_info.hh:776-855`). On Vulkan, descriptor bindings are assigned **densely in declaration order** — subpass inputs first, then pass/batch/geometry resources — and the declared slot is used only to look up the resource in the state manager (`source/blender/gpu/vulkan/vk_shader_interface.cc:199-257`). Images get a `+512` namespace offset only to keep the slot arrays disjoint (`vk_state_manager.hh:26-33`). There is exactly **one** descriptor set per shader (`vk_descriptor_set.hh:29-36`, `vk_shader.cc:667-676`), even though `Frequency` is documented as "maps to different descriptor sets" (`gpu_shader_create_info.hh:545-551`). `VkDescriptorSetLayout` is cached by binding types (`vk_descriptor_set_layouts.cc:25-62`); `VkPipelineLayout` is **not** interned and is created per shader (`vk_shader.cc:667-697`). A new set is allocated whenever the state manager is dirty or the layout changes (`vk_descriptor_set.cc:337-360`); pools have hard-coded sizes, are created with `FREE_DESCRIPTOR_SET_BIT`, never free a set, and grow forever (`vk_descriptor_pools.hh:35-41`, `vk_descriptor_pools.cc:30-38,84-111`). Push constants are std430 with a std140 UBO fallback when they exceed `maxPushConstantsSize` (`vk_push_constants.cc:52-94`).

**Tradeoff.** Declaration-order bindings are easy to author and make the shader source and the descriptor layout agree by construction, but they are fragile: inserting a resource shifts every later binding and invalidates cached SPIR-V and PSOs. One set means any change rewrites the whole set per draw; the `Frequency` enum shows the intended fix but was never implemented. The grow-forever pool is a leak with a bounded object graph — invisible in a viewport session, fatal in a long-running editor.

**Recommendation — ADAPT.** syx_aether already has the better model: explicit `(set, binding)` from Slang reflection (`src/rhi/shader/reflect.rs:133-146`), with multi-set support visible in the shader sources (`data/shaders/simple_lit.slang:27-29`). Keep explicit sets, adopt Blender's set-layout cache, add pipeline layout interning, and replace Blender's per-dirty-state set allocation with frame-pooled sets. REJECT slot indirection, `+512`, and one-set-only.

**Changes in syx_aether.**

- Make `DescriptorSet` layout-bound: `DescriptorSet::new(&DescriptorSetLayout)` instead of `DescriptorSet::new()` (`src/rhi/descriptor.rs:15-19`). Typed writes: `write_uniform_buffer(binding, &buffer, range)`, `write_sampled_image(binding, &view, &sampler)`, `write_storage_buffer(binding, &buffer)`, `write_storage_image(binding, &view)`.
- Create `DescriptorSetLayout` once per reflected interface and cache by the canonical sorted binding list `(set, binding, type, count, stage_flags)`. Create `PipelineLayout` once per tuple of set layouts + push ranges and intern it. Blender caches the first and not the second (`vk_descriptor_set_layouts.cc:25-62` vs `vk_shader.cc:667-697`), which prevents cross-shader PSO sharing.
- Use set numbers as an update-frequency convention, not as an RHI concept: set 0 = frame/view (updated once per frame), set 1 = material (updated on material change), set 2 = per-draw only when push constants cannot carry the data. This is what `Frequency` intended and never became. It also matches syx's existing `simple_lit` layout.
- Bind descriptor sets against a layout, not a pipeline. `Command::BindDescriptorSets` currently carries `pipeline: &Pipeline` (`src/rhi/command.rs:182-186`); make the encoder track the bound pipeline layout and validate at draw time, or pass `&PipelineLayout`.
- Descriptor pools: allocate per frame in flight, reset in bulk with `vkResetDescriptorPool` after the frame's `SyncPoint` completes (`vk_descriptor_pools.cc:30-38` is the call Blender never makes), or use `VK_EXT_descriptor_buffer` later. Do not allocate a set per state change.
- **Teach the planner about descriptors.** `analyze` currently returns `"descriptor resource introspection is not implemented yet"` for any non-empty `BindDescriptorSets` (`src/rhi/resource.rs:1203-1207`). Store the written resources on the set as `(ObjectId, Access, range)` and have the planner treat a bound set as a list of shader reads/writes. Blender's `VKResourceAccessInfo` is the direct precedent: binding a set populates the access info used by the render graph (`vk_descriptor_set.cc:74-278`). This is a P0 blocker, not a refinement.
- Decide combined vs separate samplers explicitly. Slang reflects `SamplerState` separately (`src/rhi/shader/reflect.rs:197`); Blender uses `COMBINED_IMAGE_SAMPLER` (`vk_shader_interface.cc:432-434`). Prefer separate `SAMPLED_IMAGE` + `SAMPLER`, because Metal has no combined type and it avoids sampler permutations; encode the choice in `DescriptorType` so the layout builder is unambiguous.
- Validate at pipeline creation: unique `(set, binding)`, stage-flag compatibility, push size against `maxPushConstantsSize`, descriptor count against limits. Blender only validates create-info consistency (`gpu_shader_create_info.cc:244-320`) and defers the rest to the driver.

```rust
pub struct DescriptorSetLayout { /* canonical bindings, frozen */ }
pub struct PipelineLayout { sets: Vec<Arc<DescriptorSetLayout>>, push: Vec<PushConstantRange> }

impl DescriptorSet {
    pub fn new(layout: Arc<DescriptorSetLayout>) -> RhiResult<Self>;
    pub fn write_uniform_buffer(&mut self, binding: u32, buffer: &Buffer, range: Range<u64>);
    pub fn write_sampled_image(&mut self, binding: u32, view: &ImageView, sampler: &Sampler);
    pub(crate) fn written_resources(&self) -> &[(ObjectId, Access, ResourceRange)]; // for the planner
}
```

## 9. Shader authoring and compilation

**Blender.** Shaders are GLSL, preprocessed at build time into C arrays with a flattened include dependency graph (`source/blender/gpu/intern/gpu_shader_dependency.cc:357-405`); a declarative `ShaderCreateInfo` describes inputs, outputs, resources, push constants, specialization constants, defines, and sources (`gpu_shader_create_info.hh:634-895`). `ShaderCompiler::compile()` is backend-agnostic and backends inject declarations via `resources_declare()` / `*_interface_declare()` (`source/blender/gpu/intern/gpu_shader.cc:884-1015`). Vulkan compiles with shaderc to SPIR-V and caches to disk keyed by a hash of the combined source (`vk_shader_compiler.cc:198-229`, `vk_shader_module.cc:52-61`); Metal translates the same GLSL to MSL and compiles at runtime (`mtl_shader.mm:273-437`); OpenGL needs a subprocess because drivers block on link. Errors are printed to stderr with source context (`gpu_shader.cc:891-895`).

**Tradeoff.** One source language for every backend is a large operational win, and the create-info makes the shader/descriptor contract explicit. But it duplicates information: the shader source already declares bindings and stage I/O, and the create-info restates them by hand, with non-owning `StringRefNull` fields (`gpu_shader_create_info.hh:630-633`) and a hand-maintained `operator==` that omits `early_fragment_test_`, `depth_write_`, and the resource-location flags (`gpu_shader_create_info.hh:1331-1359`). GLSL→MSL translation is also a permanent source of backend-specific bugs.

**Recommendation — ADAPT.** Keep Slang and keep reflection authoritative. Adopt Blender's *information model* — a frozen interface object that answers "what bindings, ranges, vertex inputs, outputs and specialization constants does this program have?" — but **generate** it from reflection at build time rather than hand-writing it. REJECT a parallel create-info DSL for hand-written shaders, and REJECT GLSL→MSL translation.

**Changes in syx_aether.**

- Generate, per shader, an `impl ShaderInterface` containing bindings, push ranges, vertex inputs, fragment outputs/attachment formats, and spec constants. Add `#[repr(C)]` push-constant structs with `offset_of!`/`size_of` const assertions. The information model is Blender's; the source of truth is Slang's.
- **Fill the reflection gaps first.** Today reflection covers bindings, push ranges, and UBO fields only (`src/rhi/shader/reflect.rs:295-405`); vertex inputs, fragment outputs, and spec constants are absent. A pipeline descriptor needs all three. This is P0.
- Fix silent drops: bindless spaces `>= 2` are skipped (`src/rhi/shader/reflect.rs:312-316`), and stage-less globals inherit the union of all entry-point stages (`:295-305`), which makes descriptor stage flags conservative. Either support bindless explicitly or error; do not silently omit a binding, because the layout will be missing it and the failure will surface at draw time.
- Keep a builder DSL only for runtime-generated shaders (material/node graphs), which is exactly Blender's `GPUCodegenCreateInfo : ShaderCreateInfo` case (`source/blender/gpu/intern/gpu_codegen.cc:59-89`). Hand-written shaders should not have a second declaration.
- Push constants: keep `[[vk::push_constant]]`, reflect offset/size/stages, and fail pipeline creation if the size exceeds the device limit. Do not copy Blender's silent UBO fallback (`vk_push_constants.cc:52-94`): changing storage changes the shader ABI and the descriptor layout, and callers cannot see it.
- Add `SHADER_ABI_VERSION` to every artifact and pipeline cache key. Blender's SPIR-V cache keys on source only (`vk_shader_module.cc:52-61`) while its pipeline cache has a full header with a build hash and `pipelineCacheUUID` (`vk_pipeline_pool.cc:681-707`); the asymmetry means a compiler upgrade reuses stale SPIR-V.
- Diagnostics: return `ShaderDiagnostic { severity, file, line, column, message }` from Slang instead of `RhiError::Shader(String)` (`src/rhi/error.rs:7-16`); keep a printable summary for the app. Blender's log parsers are good at producing source context (`gpu_shader.cc:891-895`, `vk_shader.cc:639-651`), but the caller only sees `nullptr`.

## 10. PSO caching

**Blender.** `GPUShader` is a program + interface + layouts; it is not a PSO. `VKShader::bind()` is intentionally empty (`source/blender/gpu/vulkan/vk_shader.cc:742-748`), and the pipeline is resolved at draw time from the current shader, state, and framebuffer (`vk_context.cc:279-321`). The key includes the vertex layout, shader modules, viewport/scissor arrays, attachment formats, `GPUState`, `GPUStateMutable`, the pipeline layout, and specialization values (`vk_pipeline_pool.hh:48-221`). The Vulkan backend contains **no** `VK_DYNAMIC_STATE_*` and no `vkCmdSet*`, so viewport, scissor, blend, depth, and stencil are baked into the PSO (`vk_pipeline_pool.cc:218-630`); every viewport size and scissor rectangle creates a pipeline (`:290-298`). `VertexIn` and `FragmentOut` compare only 64-bit hashes (`vk_pipeline_pool.hh:54-62,154-175`); the key contains raw module and layout handles, so identical pipelines across shaders never share (`vk_pipeline_pool.cc` get-or-create paths); the caches are unbounded `Map`s (`vk_pipeline_pool.hh:275-276`) and `remove()` has no callers (`vk_pipeline_pool.cc:632-675`). Two `VkPipelineCache` objects exist; only the static one is persisted, validated by a header containing magic, Blender version, build hash, vendor/device/ driver, and `pipelineCacheUUID` (`vk_pipeline_pool.cc:681-804`). `VKShader::warm_cache()` is `NOT_YET_IMPLEMENTED` (`vk_shader.cc:572-575`).

**Tradeoff.** Lazy creation avoids compiling permutations nobody draws, at the cost of first-use hitches. Baking all state makes draw-time binding trivial but explodes the cache; hash-only equality and handle identity make the explosion worse and can silently alias different pipelines. The complete-state key is the right idea; the implementation is a cautionary tale.

**Recommendation — ADAPT.** Adopt the complete-state key and the persistent driver cache, including its validation header. Adapt with dynamic state, interned stable IDs, a bounded cache, and async creation. REJECT hash-only equality, raw-handle identity, unbounded caches, and per-viewport PSOs.

**Changes in syx_aether.**

- `PipelineKey { shader: ShaderId, vertex_layout: VertexLayoutId, attachments: AttachmentKey, state: GraphicsState, spec: Box<[u32]>, layout: PipelineLayoutId }`, with derived `Hash`/`Eq` over every field. `AttachmentKey` includes color formats, depth/stencil formats, and `sample_count`.
- Dynamic state: viewport, scissor, blend constants, stencil reference, depth bias, and (core 1.3 extended dynamic state) cull mode, front face, depth test/write/compare, primitive topology. syx_aether already records `SetViewport`, `SetScissor`, `SetBlendConstants`, and `SetStencilReference` (`src/rhi/command.rs:193-199`, `src/rhi/vk/command.rs:458-496`); make the pipeline not contain them. Blender's `GPUStateMutable` includes point size, line width, and depth range, which are not Vulkan pipeline state (`source/blender/gpu/intern/gpu_state_private.hh:81-100`) — an example of over-keying.
- Intern `ShaderModule` by content hash and `PipelineLayout` by canonical layout, so keys use stable IDs and identical pipelines share. Blender's key uses `VkShaderModule`/`VkPipelineLayout` handles and creates a layout per shader, so cross-shader sharing never happens.
- Bounded cache: `HashMap<PipelineKey, Arc<Pipeline>>` with an LRU cap and an explicit destroy path; do not wait for device teardown. Blender's map grows for the process lifetime and its removal function is dead.
- Persist `VkPipelineCache` with a header: magic, app version, build hash, `pipelineCacheUUID`, driver version, and `SHADER_ABI_VERSION`. Ignore on mismatch (Blender's header is a good template, `vk_pipeline_pool.cc:681-707`). Persist *all* pipelines, not only statically compiled ones — Blender only serializes the static cache (`:721-804`), so material PSOs recompile every run.
- Async creation: worker threads with per-thread `VkPipelineCache` objects (the Vulkan object requires external synchronisation), merged at frame end. `PipelineCache::get_or_create` should return either a ready pipeline or a pending handle; the encoder must reject a pipeline that is not ready. Blender serializes all creation behind one mutex (`vk_pipeline_pool.cc:180,222`) and has no pre-warm on Vulkan.
- Add `PipelineCache::prewarm(&[PipelineKey])` for the last N keys per shader. Blender only does this on Metal (`mtl_shader.mm:716-744`).

```rust
pub struct PipelineKey {
    pub shader: ShaderId,
    pub vertex_layout: VertexLayoutId,
    pub attachments: AttachmentKey, // formats + sample_count
    pub state: GraphicsState,       // only state not covered by dynamic state
    pub spec: Box<[u32]>,
    pub layout: PipelineLayoutId,
}

impl PipelineCache {
    pub fn get(&self, key: &PipelineKey) -> Option<Arc<Pipeline>>;
    pub fn get_or_create(&self, key: &PipelineKey, desc: &PipelineDesc<'_>)
        -> RhiResult<Arc<Pipeline>>;
    pub fn prewarm(&self, keys: &[PipelineKey]);
}
```

## 11. Draw submission and multi-draw

**Blender.** The draw manager builds a frame-local structure-of-arrays database indexed by a 31-bit `ResourceHandle` whose high bit encodes inverted handedness (`source/blender/draw/intern/draw_handle.hh:10-51`). A compute pass frustum-culls into a visibility bitmask (`draw_view.cc:254-309`); a second compute pass compacts visible instances into two indirect commands per group (front/back facing) and a per-instance resource-ID buffer (`draw_command.cc:782-866`, `intern/shaders/draw_command_generate_comp.glsl:17-143`). Vertex shaders fetch `resource_id_buf[gl_BaseInstance + gl_InstanceID]`, so per-object lookup is one SSBO load and there is no per-draw uniform. `DrawMulti::execute` then issues **one indirect draw per group per facing range** (`draw_command.cc:208-250`); the multi-draw entry point exists but nothing under `source/blender/draw` calls it (`GPU_batch.hh:396-412`, `vk_batch.cc:79-124`). Batching is "same shader/state/batch as the previous command"; there is no global state sort, and draw order inside a group is non-deterministic (`draw_pass.hh:12-40`).

**Tradeoff.** GPU compaction removes per-object CPU draw calls and per-draw uniforms, and gives one place to cull; it costs a dispatch, a barrier, and an indirect buffer per pass, and per-group draw calls still scale with state segments. The resource-ID indirection is a genuinely good fit for a DCC app with heterogeneous objects and instancing.

**Recommendation — ADOPT.** Adopt the GPU visibility/compaction design at the client layer, adopt per-instance resource IDs, and adopt indirect draws. Adapt by actually using the `draw_count` range multi-draw primitive that syx_aether already has, and by keeping all of this out of the RHI. REJECT `Batch` and any draw path that reads an implicit active shader.

**Changes in syx_aether.**

- Keep `Draw`, `DrawIndexed`, `DrawIndirect`, and `DrawIndexedIndirect` as they are (`src/rhi/command.rs:110-134`); `DrawIndirect` already carries `draw_count` and `stride`, which is better than Blender's per-group calls (`src/rhi/vk/command.rs:262-290`).
- To use the range fully GPU-driven, issue one indirect call per state group and have the compaction shader write `instance_count = 0` for groups with no visible instances (or use conditional rendering). Document that `draw_count` is an upper bound and that the GPU may draw zero instances.
- Port the resource-ID convention: a per-frame SSBO indexed by `first_instance + instance_id`, filled by the compaction pass. Add a documented convention for how `SV_InstanceID` and a base-instance push constant map to the index; Blender's `draw_view_info.hh:263-269` is the reference.
- Keep the client-layer split from document 04: `src/render/frame.rs` (SoA + handle arena), `src/render/pass.rs` (`PassBuilder`/`Pass`), `src/render/view.rs` (matrices + visibility), `src/render/cull.rs` (visibility + compaction). None of these belong in `src/rhi/`.
- Sorting: merge contiguous same-state draws; provide an optional stable sort for opaque passes and a sub-pass depth sort for transparency in the client layer. Do not add a global state sorter to the RHI; Blender deliberately does not have one (`draw_pass.hh:12-40`).
- Occlusion culling: Blender only frustum-culls and its Hi-Z is not wired into object submission (`draw_visibility_comp.glsl:8`). If syx_aether needs it, put it behind the same visibility buffer interface; do not make it an RHI feature.
- The RHI should not expose a "multi-draw" abstraction separate from indirect draws. The primitive is `cmd_draw_indexed_indirect` with a count; anything else is a client concern.

## 12. Threading and async compilation

**Blender.** The frame loop takes a global "main context" mutex (`source/blender/windowmanager/intern/wm_draw.cc:1547`), the active context is thread-local (`gpu_context.cc:48,166-181`), and all Vulkan submissions go through one background serial worker on one queue (`vk_device.cc:432-530,546-558`). Shader compilation uses task-pool jobs and a batch API that waits by sleeping 1 ms in a loop (`vk_shader_compiler.cc:292-303`); Metal uses a worker pool with one GPU context per worker (`mtl_shader.mm:1643-1781`). PSO creation is serialized behind one mutex (`vk_pipeline_pool.cc:180,222`). The draw manager itself is not threaded and the code says it needs to be (`source/blender/draw/intern/draw_manager_c.cc:949-950`).

**Tradeoff.** Single-threaded submission is simple and matches an application with one main loop. It blocks multi-queue overlap, makes async compile a bolt-on, and turns every CPU-side hiccup into a frame hitch. For a reusable RHI the thread-affine `Context` is the part that hurts most: every background operation must activate a context on its thread.

**Recommendation — ADAPT.** Keep explicit `&Device`/`&Queue` ownership and the per-queue submit gate, remove the recording lease, and put compilation behind a dedicated service with pollable results. REJECT the 1 ms sleep loop and any thread-local "current context".

**Changes in syx_aether.**

- Remove the recording lease (see §5). Command pools become per-thread; `submit_buffer` already serializes the native submit with `submit_gate` (`src/rhi/vk/queue.rs:221-229`). Recording on N threads must not contend.
- `BuiltRecorder`/`CommandBuffer` should be `Send` when the backend handle is `Send`, so a worker can record and the render thread can submit. Keep the queue check (a command buffer is only submittable to its originating queue); it already exists (`src/rhi/queue.rs:137-141`).
- Compiler service: one thread owning the `GlobalSession`, or one session per worker, because Slang sessions are not `Send`/`Sync` (`src/rhi/shader/compiler.rs:1-6`). API: `Compiler::compile(file, entries) -> JobId`, `JobId::poll() -> Option<RhiResult<CompiledShader>>`, `JobId::wait()`. Use a channel + condvar; do not sleep-poll.
- Artifact cache key: `(source bytes, entry points, defines, target env, optimization, debug, compiler version, SHADER_ABI_VERSION)`. Blender keys SPIR-V on source only (`vk_shader_module.cc:52-61`).
- PSO creation on worker threads with per-thread `VkPipelineCache`; merge caches at frame end. Expose `PipelineCache::get_or_create_async`.
- Frames in flight, `FrameIndex`, and descriptor/transient reset are application-level policy. The RHI should provide `SyncPoint` and `Device::wait` (`src/rhi/vk/device.rs:419-439`) and nothing that hides a wait. Blender's three per-thread resource pools (`vk_device.hh:95-143`) are effectively one frame because present waits for completion; do not copy that.

## 13. Debugging and validation

**Blender.** Debug groups and object labels exist on Vulkan: nodes are associated with debug groups when `--debug-gpu` is set (`render_graph/vk_render_graph.hh:140-175`), descriptor sets are labelled with the shader name (`vk_descriptor_set.cc:357`), and compile logs are parsed and printed with source context (`vk_shader.cc:639-651`). `--debug-gpu-force-workarounds` forces every workaround bit for testing (`vk_backend.cc:347-367`). GPU timer queries are commented out, so profiling is mostly labels and CPU timers (`draw_manager_profiling.cc:106-117`). Errors go to stderr, and only shader/texture/offscreen creation can fail recoverably.

**Tradeoff.** Labels and validation are cheap and invaluable; stderr-only reporting is not. A global debug flag that changes behavior is useful for testing but dangerous if it leaks into production policy. Missing GPU timing makes performance work guesswork.

**Recommendation — ADOPT.** Adopt debug labels/groups, validation in debug builds, and a debug flag that forces workaround paths. Adapt the reporting into typed diagnostics and add timestamp queries. Fix syx_aether's no-op labels first — an API that silently does nothing is worse than no API.

**Changes in syx_aether.**

- Implement `begin_debug_label`/`end_debug_label`/`insert_debug_label`. They are currently empty because the device dispatch is not retained (`src/rhi/vk/command.rs:498-502`); load `vkCmdBeginDebugUtilsLabelEXT`, `vkCmdEndDebugUtilsLabelEXT`, and `vkCmdInsertDebugUtilsLabelEXT` into the device and call them. The command enum already carries labels (`src/rhi/command.rs:197-199`) and the planner validates nesting (`src/rhi/resource.rs:1208-1220`).
- Add `Device::set_object_name(ObjectId, &str)` backed by `vkSetDebugUtilsObjectNameEXT`, and label resources at creation (`BufferDesc.name` already exists, `src/rhi/types.rs:34`). Blender labels descriptor sets and immediate buffers; do the same for pipelines, sets, and command buffers.
- Typed errors: replace `RhiError::BackendError(String)` / `InvalidCommand(String)` with a struct carrying `kind`, a `&'static str` context, an optional `ObjectId`, and an optional source error (`src/rhi/error.rs:7-16`). Add `RhiError::Validation(String)` so validation-layer messages can be routed to the caller instead of only the logger.
- Timestamp queries: expose `QueryPool` and a `PassTiming` readback in the RHI (or behind a `Profile` feature). Blender's stubbed timers are a known gap; syx_aether should not repeat it.
- Validation: `DeviceSelection` should report enabled layers and the selected device; log driver version and `pipelineCacheUUID`; keep syncval on in debug (`src/rhi/vk/instance.rs:67`) and run one headless frame with it in CI.
- Workarounds: keep a per-device `Capabilities.workarounds` bit set and a test-only `Features::force_workarounds`. Never let a workaround change engine-visible behavior the way `GPU_vulkan_render_pass_workaround()` does in Workbench (`source/blender/draw/engines/workbench/workbench_state.cc:185`).

## 14. Swapchain and present

**Blender.** GHOST owns the Vulkan instance, physical device, device, queue, surface, and swapchain; the GPU module receives the handles (`vk_device.cc:87-99`) and installs pre/post present callbacks (`vk_backend.cc:490-492`). `GHOST_ContextVK::swapBuffers()` acquires with a binary fence and CPU-waits on it (`intern/ghost/intern/GHOST_ContextVK.cc:547-555`), invokes the pre-callback (which blits `back_left` into the acquired image, transitions it to `PRESENT_SRC_KHR`, and flushes with `WAIT_FOR_COMPLETION`, `vk_context.cc:349-397`), presents with **zero** wait semaphores (`GHOST_ContextVK.cc:566-579`), and recreates the swapchain on out-of-date/suboptimal. Present mode prefers FIFO despite a stale comment (`GHOST_ContextVK.cc:695-722`); the surface format is hard-picked R8G8B8A8/B8G8R8A8 (`:756-784`); image count starts at `minImageCount + 1`, is clamped to `maxImageCount` when that is nonzero, and is capped at 3 only when `minImageCount <= 3` (`:838-845`, condition at `:843`).

**Tradeoff.** Correctness-first and easy to reason about: a full wait means one frame in flight and no semaphore plumbing. It also serializes CPU and GPU, hides the submit inside the platform callback, and makes present timing independent of frame pacing. GHOST owning the swapchain is right for Blender's windowing model, but it makes the RHI unusable without GHOST.

**Recommendation — ADAPT.** Keep the layering "platform owns the surface; RHI fills an image", but make the swapchain a first-class RHI object and use semaphores for both acquire and present. Adopt 2-3 frames in flight. REJECT the CPU fence + full-frame wait + zero-wait present path.

**Changes in syx_aether.**

- Add `src/rhi/swapchain.rs` and `src/rhi/vk/swapchain.rs`. `Swapchain::new(device, surface, desc)` takes a platform surface (from `winit`/`raw-window-handle` in the app, not in the RHI).
- `acquire` returns an `AcquiredImage { image, view, index, wait }`, where `wait` is an opaque token accepted by `Queue::submit`. `present(queue, image, render_done)` makes the present wait on a GPU semaphore, not on the CPU. If the backend uses a binary render-finished semaphore for present, expose it as `submit_for_present(...) -> PresentToken`; if it can use a timeline value, keep `SyncPoint`.
- Frames in flight: 2-3. Pair each frame with a `FrameIndex`, a descriptor pool, and a transient allocator; reset only after the frame's `SyncPoint` completes.
- Recreate on `OutOfDate`/`Suboptimal`; recreate the swapchain only, never the device (Blender does this correctly: the GPU context survives resize, `wm_window.cc:847`).
- Choose surface format by querying and preferring an sRGB format; do not hard-code. Present mode: FIFO is a safe default, MAILBOX opt-in.
- Keep "render offscreen, then composite/blit" at the client layer. Blender's `back_left` model (`vk_context.cc:38-106`) is a good pattern for editor regions; it does not belong in the swapchain object.

```rust
pub struct SwapchainDesc {
    pub width: u32,
    pub height: u32,
    pub format: Format,
    pub present_mode: PresentMode,
    pub frames_in_flight: u32,
}

pub struct AcquiredImage {
    pub image: Image,
    pub view: ImageView,
    pub index: u32,
    pub wait: AcquiredToken, // opaque; accepted by Queue::submit
}

impl Swapchain {
    pub fn acquire(&mut self, timeout: Option<Duration>) -> RhiResult<AcquiredImage>;
    pub fn present(&mut self, queue: &Queue, image: AcquiredImage, done: SyncPoint)
        -> RhiResult<PresentOutcome>; // Success | Suboptimal | OutOfDate
    pub fn reconfigure(&mut self, width: u32, height: u32) -> RhiResult<()>;
}
```

## 15. Metal readiness gate

Metal is not implemented now, but the abstraction must not make it impossible. The gate is short: everything below must already be true before a Metal backend is attempted.

| Rule | Why it matters for Metal | Current syx_aether risk |
|---|---|---|
| No `VkImageLayout` in public types; transitions carry access + intent | Metal has no layouts; transitions are execution/memory dependencies or encoder boundaries | Already true (`src/rhi/resource.rs:467-474`, layout mapping private in `src/rhi/vk/command.rs:593-603`) |
| Descriptors are `(set, binding, type)` intent, not Vulkan structs | Metal binds encoder-local slots or argument buffers; set indices cannot leak | True at the reflection layer; the stub API must not take `VkDescriptorSetLayout` |
| `SyncPoint` is opaque; queues own their timelines | Metal uses `MTLSharedEvent` | Currently leaks a `VkSemaphore` (`src/rhi/vk/queue.rs:231`); fix |
| Pipelines are immutable complete descriptions; state is dynamic where possible | Metal bakes most pipeline state into the PSO but exposes viewport, scissor, blend constants, and stencil reference on the encoder; a complete descriptor with dynamic state maps cleanly | `PipelineDesc` does not exist yet |
| No geometry/tessellation/transform feedback in the core | Metal has no geometry shaders; Blender injects them as a workaround (`vk_shader.cc:1226-1309`) | Stage enums already include them (`src/rhi/shader/reflect.rs:36-44`, `src/rhi/vk/command.rs:707-728`); keep them reflection-only and do not build pipeline paths |
| Frame scope is explicit but backend-neutral | Metal needs an autorelease pool per frame; Vulkan needs a frame index | No frame object yet; design it without submission inside |
| Present uses semaphores, not CPU waits | Metal present is drawable/command-buffer based | Design requirement in §14 |

Blender's Metal backend is useful as a translation reference (`mtl_command_buffer.mm`, `mtl_memory.hh`, `mtl_shader.hh`), not as an API model: it implements the same GL-shaped surface, including global binding state and one active encoder of each kind.

## 16. Prioritized roadmap

Ordering is by dependency and by what unblocks a real frame. "P0" means the RHI cannot render a textured, lit triangle without it.

### P0 — vertical slice

| # | Deliverable | Files | Acceptance |
|---|---|---|---|
| 1 | `PipelineDesc` + `PipelineKey` + `PipelineLayout`/`DescriptorSetLayout` creation from reflection | `src/rhi/pipeline.rs`, `src/rhi/vk/pipeline.rs`, `src/rhi/vk/device.rs:366-372` | A pipeline is created from a `ShaderInterface` and a `GraphicsState`; key equality is field-based |
| 2 | Descriptor set creation/writes; planner introspection of bound sets | `src/rhi/descriptor.rs`, `src/rhi/resource.rs:1203-1207` | A shader with set 0 + set 1 records and draws without the "introspection" error |
| 3 | Reflection of vertex inputs, fragment outputs, specialization constants | `src/rhi/shader/reflect.rs` | A pipeline can be built from reflection alone, no hand-written vertex layout |
| 4 | Upload path: `Buffer::write`, `Device::upload_buffer`, staging ring, frame allocator | `src/rhi/vk/resource.rs`, `src/rhi/vk/allocator.rs` | A texture and a UBO can be filled without per-upload VMA allocation |
| 5 | Device-owned retire queue, non-panicking `Drop`, concurrent reservations | `src/rhi/lifetime.rs`, `src/rhi/resource.rs`, `src/rhi/pipeline.rs`, `src/rhi/descriptor.rs` | Two threads can record commands referencing the same buffer; teardown never panics |
| 6 | Swapchain acquire/present with semaphores, 2 frames in flight | `src/rhi/swapchain.rs`, `src/rhi/vk/swapchain.rs` | A window clears to a color and presents without a full-frame CPU wait |
| 7 | Real debug labels + object names | `src/rhi/vk/command.rs:498-502`, `src/rhi/vk/device.rs` | RenderDoc captures show pass and object names |

### P1 — usable engine RHI

| # | Deliverable | Files | Acceptance |
|---|---|---|---|
| 8 | Pipeline cache: bounded LRU, persistent `VkPipelineCache` with header, prewarm | `src/rhi/vk/pipeline.rs` (new cache module) | Second run reuses PSOs; cache validates driver/build hash |
| 9 | `MemoryClass`, `sample_count`, input-attachment/transient usage | `src/rhi/types.rs`, `src/rhi/vk/resource.rs` | MSAA render target and input attachment are expressible |
| 10 | Barrier merge pass + stage-precise `Access`; opaque `SyncPoint` | `src/rhi/resource.rs`, `src/rhi/vk/command.rs`, `src/rhi/sync.rs` | Consecutive barriers merge; no Vulkan handle in public types |
| 11 | Async compile service + generated `ShaderInterface` | `src/rhi/shader/compiler.rs`, build script | Compile jobs are pollable; no sleep loop; interface is generated |
| 12 | Client-layer frame/pass/view/cull modules | `src/render/frame.rs`, `pass.rs`, `view.rs`, `cull.rs` | A scene draws through the client layer without new RHI concepts |

### P2 — scale and robustness

| # | Deliverable | Files | Acceptance |
|---|---|---|---|
| 13 | Descriptor buffers / push descriptors, bindless | `src/rhi/descriptor.rs`, `src/rhi/vk/descriptor.rs` | Bindless set works; no per-draw set updates |
| 14 | Multi-queue overlap, async compute | `src/rhi/queue.rs`, `src/rhi/resource.rs` | Cross-queue release/acquire is exercised in a test |
| 15 | Metal backend behind the same traits | `src/rhi/backend.rs`, new `src/rhi/metal/**` | Triangle renders; no Vulkan types in the trait |
| 16 | Timestamp queries, pass timings, headless golden tests | `src/rhi/query.rs` (new), `tests/` | Per-pass GPU timings and a validation-clean headless frame |

## 17. Do not copy

Each item is a Blender mechanism that looks convenient and costs more than it saves.

1. **`Batch` as an object, and `Batch::shader`.** Geometry grouping belongs to the client; a shader pointer in a geometry object recreates the active-shader model (`GPU_batch.hh:91-92,269-271`, `gpu_batch.cc:413-423`).
2. **Immediate mode (`imm*`).** A second draw path with its own buffering, format conversion, and wide-line shader swapping (`gpu_immediate.cc:118-192`). A UI geometry builder that emits into the explicit stream covers the use case.
3. **A global state manager and `GPU_state_*`.** A mutable 64-bit bitfield plus call-order-dependent setters (`gpu_state_private.hh:23-100`, `gpu_state.cc:28-37`).
4. **Active shader / active framebuffer in a context.** `GPU_shader_bind` mutates a thread-local context and also rebinds matrices and sRGB state (`gpu_shader.cc:431-461`).
5. **Binding by name or texture unit.** `GPU_shader_get_sampler_binding` + `GPU_texture_bind(tex, unit)` (`gpu_shader.cc:657-662`, `gpu_texture.cc:558-569`); the `+512` slot namespace (`vk_state_manager.hh:26-33`) and dense declaration-order bindings (`vk_shader_interface.cc:199-257`).
6. **`Frequency` as a fake set split.** Documented as "maps to different descriptor sets" and never implemented (`gpu_shader_create_info.hh:545-551`).
7. **A hand-written create-info that duplicates the shader.** Non-owning `StringRefNull` fields (`gpu_shader_create_info.hh:630-633`) and a hand-maintained `operator==` that omits fields (`:1331-1359`).
8. **No dynamic state.** PSOs per viewport size and scissor rectangle (`vk_pipeline_pool.cc:290-298`); `GPUStateMutable` over-keys point size, line width, and depth range (`gpu_state_private.hh:81-100`).
9. **Hash-only PSO key equality and raw-handle identity.** `VertexIn`/`FragmentOut` compare `hash() == hash()` (`vk_pipeline_pool.hh:54-62,154-175`); keys contain module/layout handles, so identical pipelines never share.
10. **Unbounded PSO caches and dead removal paths.** `Map`s grow for the process lifetime; `remove()` has no callers (`vk_pipeline_pool.hh:275-276`, `vk_pipeline_pool.cc:632-675`).
11. **Descriptor pools that only grow.** Hard-coded sizes, `FREE_DESCRIPTOR_SET_BIT` with no free, and a `reset()` that is never called (`vk_descriptor_pools.hh:35-41`, `vk_descriptor_pools.cc:30-38,84-111`).
12. **Per-upload VMA allocation and staging through the discard pool.** `vk_staging_buffer.cc:14-33`; use a staging ring and a frame allocator.
13. **Per-draw descriptor set allocation.** A new set whenever the state manager is dirty (`vk_descriptor_set.cc:337-360`); pool per frame and reset in bulk.
14. **One descriptor set for everything.** Forces whole-set rewrites and blocks layout sharing (`vk_descriptor_set.hh:29-36`).
15. **CPU-synchronized present with zero wait semaphores.** `vk_context.cc:394-395`, `GHOST_ContextVK.cc:566-579`; use a render-finished semaphore and 2-3 frames in flight.
16. **`render_begin`/`render_step`/`render_end` as a global frame protocol.** Semantics differ per backend and `render_step` is an autorelease-pool hook on Metal only (`mtl_backend.mm:106-157`, `vk_backend.cc:551-575`).
17. **A dummy backend.** `DummyBackend` hides feature gaps (`gpu_context.cc:387-389`); use trait-level mocks.
18. **Driver blacklists inside the RHI, duplicated across layers.** `vk_ghost_api.hh:15-28`; make it application policy.
19. **Thread-local `Context::get()` and a global draw-loop mutex.** `gpu_context.cc:48`, `wm_draw.cc:1547`; pass `&Device`/`&Queue` and let the app own its frame loop.
20. **`Arc::strong_count` as a liveness test and panicking `Drop`.** syx_aether-specific: retire explicitly and never panic in `Drop` (`src/rhi/lifetime.rs:84-93`, `src/rhi/resource.rs:253-260`).

## Appendix A. Corrections to the study reports

Source wins over the reports. These are the places where the decision document deliberately departs from a report's wording or recommendation.

1. **Document 01, R8, argues that borrowing resources in `Command` cannot express deferred submission and makes commands non-`Send`.** Partly wrong. `Recorder::build()` records eagerly and returns `BuiltRecorder<'queue>`, which contains the native command buffer and no `Command` borrows (`src/rhi/queue.rs:418-468`). The borrow is confined to the builder, not to the recorded artifact. What is true is that `BuiltRecorder` borrows the *queue* and holds the queue lease; the fix is to own the command buffer, not to replace borrows with IDs. Recommendation updated in §5 and §12.
2. **Document 01, R10, recommends keeping syx's queue lease.** Disagreed. The lease makes recording single-threaded (`src/rhi/queue.rs:95-103`). Blender can afford a single submission thread; syx_aether cannot, and the native submit gate (`src/rhi/vk/queue.rs:221-229`) is the only serialization that is actually required.
3. **Document 01, R9: `Drop` panics if the manager is missing.** Confirmed, and extended: `DiscardManager::retire` also calls `LifetimeTracker::with(...).expect(...)` (`src/rhi/lifetime.rs:146-153`). The recommended fix is a device-owned queue captured at creation, not a different global initialization order.
4. **Document 02, §4.5 and §11.2: "no transient/linear allocator."** Confirmed for Blender, but note Blender does have a special-purpose mapped buffer for immediate mode (`vk_immediate.cc:139-175`) and Metal has a scratch ring (`mtl_memory.hh:469-524`). The general mechanism is missing, not the idea.
5. **Document 03, §5.3: "no dynamic state."** Confirmed by grep over `source/blender/gpu/vulkan`: no `VK_DYNAMIC_STATE_*` and no `vkCmdSet*`. The consequence (a PSO per viewport/scissor) is the strongest argument for syx_aether's already-present dynamic-state commands.
6. **Document 03, §4.8 and document 02, §6.2: `Frequency` / single set.** Both confirmed: `Frequency` is only an ordering hint in 4.4 (`vk_shader_interface.cc:47-50`) and one set is created (`vk_shader.cc:667-676`). The reports' recommendation to split by frequency is adopted here as a *layout convention*, not as an RHI concept.
7. **Document 04, §1.4: "one indirect draw per group, not multi-draw."** Confirmed. `DrawMulti::execute` calls `GPU_batch_draw_indirect` per group and facing range (`draw_command.cc:208-250`); nothing under `source/blender/draw` uses the multi-draw entry point (`GPU_batch.hh:396-412`, `vk_batch.cc:79-124`). The task's named `draw_shgroup.cc`, `draw_pass.cc`, and `GPU_multi_draw` do not exist in this tree; the report's correction stands.
8. **Document 02, §2.2: "no secondary-buffer path anywhere."** The only allocation site uses `VK_COMMAND_BUFFER_LEVEL_PRIMARY` (`vk_device.cc:473-480`). The stronger claim ("anywhere in the directory") was not re-verified line by line here; the practical conclusion (primary-only) is correct.
9. **Line numbers.** This checkout contains local instrumentation, so a few report line numbers point at `std::clog`/`std::stacktrace` lines rather than upstream logic. Anchors in this document were re-checked against the checkout; where a range is broad, it covers the construct rather than a single statement.

## Appendix B. What "done" looks like

A frame that satisfies this document looks like:

1. The app creates a `Device` from a `DeviceDesc` and receives a `DeviceSelection` naming the adapter and driver.
2. Shaders are compiled by a compiler service into `ShaderModule`s with generated `ShaderInterface`s.
3. `PipelineLayout` and `DescriptorSetLayout` are interned; `Pipeline`s are created from complete `PipelineDesc`s with dynamic state and cached by a field-based key.
4. Per-frame data goes into a frame allocator; textures upload through a staging ring; descriptor sets come from a per-frame pool.
5. A `Recorder` on the render thread records explicit passes, draws, and copies; `build()` plans barriers; `submit()` returns a `SyncPoint`; nothing submits behind the caller's back.
6. The swapchain acquires with a semaphore, renders into an offscreen target, presents with a render-finished semaphore, and keeps two frames in flight.
7. Debug labels name every pass and every resource; validation and syncval are clean in debug; a teardown drops everything without panicking and without a full-device wait in normal operation.

If a proposed change makes any of those seven steps require a global, a thread-local, a hidden wait, or a Vulkan type in the public API, it is the wrong change.
