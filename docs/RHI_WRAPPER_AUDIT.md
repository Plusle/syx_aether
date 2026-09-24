# RHI wrapper + `Arc<dyn Rhi*>` 模式审查

## 约定

所有具有后端实现、并且会跨公开 API 生命周期存在的 RHI 对象，统一采用：

```rust
pub struct Object {
    inner: Arc<dyn RhiObject>,
}
```

公开 concrete wrapper 负责用户接口和所有权；`Rhi*` trait、trait object 与 native backend 对象只在 crate 内部使用。

## 审查清单

| 优先级 | 对象或区域 | 当前问题 | 建议 |
| --- | --- | --- | --- |
| P0（已完成） | `SyncPoint` | 公开构造和读取裸 `timeline: u64`，允许用户伪造并泄漏 native handle | 保持轻量 `Copy` 值类型，但加入 `QueueId` 并收紧构造与 native accessor |
| P1（已完成） | `rhi::vk` | Vulkan 模块以及 `Instance`、`Device`、`VulkanDevice` 对外公开 | 将 Vulkan 实现层收紧为 crate-private |
| P1（已完成） | 资源 wrapper 接口 | `Buffer::size/name` 等能力只存在于 private trait，外部用户无法调用 | 在 concrete wrapper 上提供公开固有方法并转发 |
| P2（已完成） | tracker 类型 | `ObjectId`、`ResourceRange`、`ResourceTracker` 被公开，但只服务于内部 barrier/lifetime 系统 | 全部收紧为 crate-private |
| P2（已完成） | wrapper 内部接口 | `Sampler::inner()` 的可见性与其他 wrapper 不一致 | 统一为 `pub(crate) fn inner()` |

## 1. [已完成] `SyncPoint` 与 timeline

原先 `SyncPoint` 保存：

```rust
pub struct SyncPoint {
    timeline: u64,
    value: u64,
}
```

这里的 `timeline` 实际上是 Vulkan semaphore raw handle，并不是 backend-neutral 概念。公开的 `SyncPoint::new()` 允许用户构造不存在的 timeline；queue submit 也会直接将 wait point 转换成 Vulkan semaphore，无法可靠拒绝来自其他 backend 的对象。

`SyncPoint` 是轻量提交凭证，而不是独立拥有 native timeline 生命周期的 GPU 对象，因此不采用 `Arc<dyn RhiTimeline>`。已接受的结构是：

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SyncPoint {
    queue: QueueId,
    timeline: u64,
    value: u64,
}

impl SyncPoint {
    pub(crate) const fn new(queue: QueueId, timeline: u64, value: u64) -> Self;
    pub(crate) const fn queue(self) -> QueueId;
    pub(crate) const fn timeline(self) -> u64;
    pub const fn value(self) -> u64;
}
```

规则如下：

- 用户不能构造 `SyncPoint`，也不能读取 native timeline；
- `value()` 是唯一公开查询接口；
- `QueueId` 在进程内不复用，用于拒绝旧 backend 或 foreign queue 的 point；
- `VulkanDevice` 在 backend 存活期间始终持有所有 queue，因此 SyncPoint 不负责延长 timeline 生命周期；
- backend `terminate()` 后，属于该 backend 的全部 SyncPoint 都失效。

实施内容：

- 新增 `RhiError::InvalidSyncPoint(String)`；
- `VkCommandQueue` 生成 point 时写入自身 `QueueId`；
- `VkQueueRegistry` 提供 QueueId 所属关系检查；
- queue submit 在创建 native wait info 前验证所有 wait points；
- `VulkanDevice::wait` 始终验证 QueueId，不再只在 debug 构建验证 raw semaphore；
- `VulkanDevice` 保存共享 queue registry，并移除 timeline `HashSet`；
- lifetime tracker 的 last-use map 改为使用 `QueueId` 作为 key，同一队列的新 point 覆盖旧 point。

验证需要覆盖 known/foreign QueueId、旧 backend point、同一队列 last-use 合并，以及 submit 在调用 Vulkan API 前拒绝无效 wait point。

## 2. [已完成] 隐藏 Vulkan 实现层

原先 `rhi.rs` 使用：

```rust
pub mod vk;
```

并且 `Instance`、`Device`、`VulkanDevice` 是公开类型，`Device` 还公开了 `ash::Device`、queue family 等字段。外部代码可以因此绕过 `Backend` wrapper。

现已改为：

```rust
pub(crate) mod vk;
```

Vulkan 子模块、`Instance`、`Device`、`VulkanDevice`、它们的构造函数和 `collect_garbage()` 也已收紧为 crate-private；native device 和 feature 字段不再对外公开。

外部用户只接触：

- `Backend`
- `Queue`
- `Buffer`、`Image`、`ImageView`、`Sampler`
- `Pipeline`、`DescriptorSet`
- `SyncPoint`
- `Recorder`、`BuiltRecorder`

如果以后需要 Vulkan native interop，应设计显式 extension trait 或 feature，而不是公开整个 backend 实现目录。

## 3. [已完成] 补齐 wrapper 固有方法

部分 wrapper 已经持有 `Arc<dyn Rhi*>`，但用户需要的查询能力仍然只存在于 crate-private trait。

现已增加：

```rust
impl Buffer {
    pub fn name(&self) -> &str;
    pub fn size(&self) -> u64;
}

impl Image {
    pub fn subresource_info(&self) -> ImageSubresourceInfo;
}

impl ImageView {
    pub fn subresource_range(&self) -> ImageSubresourceRange;
}
```

暂时不公开 `ImageView::image()`、`ObjectId` 或任何 `inner()`；这些能力目前只供 tracker 与 backend 解包使用。

`Sampler`、`Pipeline`、`DescriptorSet` 当前没有需要公开查询的 backend-neutral 属性，所以暂时只保留构造和生命周期接口。

## 4. [已完成] 收紧 tracker 类型

以下类型仅服务于自动 barrier 和 lifetime 系统，不属于用户 API：

- `ObjectId`
- `ResourceRange`
- `ResourceTracker`
- `lifetime` 模块中的 manager、reservation 和内部状态

现已使用 crate-private 可见性：

```rust
pub(crate) struct ObjectId(...);
pub(crate) enum ResourceRange { ... }
pub(crate) struct ResourceTracker { ... }
```

并将 `lifetime` 模块设为 crate-private。Recorder 不公开 barrier，因此用户没有理由直接操作这些状态类型。

## 5. 已符合模式的对象

以下结构当前方向正确：

- `Backend -> Arc<dyn RhiBackend>`
- `Queue -> Arc<dyn RhiQueue>`
- `Buffer -> Arc<dyn RhiBuffer>`
- `Image -> Arc<dyn RhiImage>`
- `ImageView -> Arc<dyn RhiImageView>`
- `Sampler -> Arc<dyn RhiSampler>`
- `Pipeline -> Arc<dyn RhiPipeline>`
- `DescriptorSet -> Arc<dyn RhiDescriptorSet>`

`RhiCommandBuffer` 是合理例外：它完全由 Queue/Recorder 内部租赁和回收，没有直接暴露给用户，因此不需要额外的公开 `CommandBuffer` wrapper。

所有 wrapper 的 `inner()` 均只在 crate 内使用；`Sampler::inner()` 已与其他资源 wrapper 统一为 `pub(crate)`。

Shader compiler 与 reflection 也不属于 backend-owned GPU object，应继续保持普通 value/service 类型。

## 审查状态

本文列出的 wrapper、SyncPoint、Vulkan implementation exposure 和内部 tracker 可见性问题均已完成。
