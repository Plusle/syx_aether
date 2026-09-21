# RHI Queue / Recorder 完整审查

审查基线：提交 `38dd2fb`（`feat(rhi): add queue recorder lifecycle`）

验证结果：

- `cargo +nightly fmt --check`：通过
- `cargo check`：通过
- `cargo test --quiet`：32 个测试通过
- `cargo clippy --all-targets --all-features -- -D warnings`：失败，现有 `src/util/log.rs` 存在 clippy 警告

## 结论摘要

Recorder、队列独占 lease、BuiltRecorder typestate、全局资源追踪器和 Vulkan command-buffer 回收流程整体结构清晰，基础测试覆盖也足够验证主要生命周期。

不过，在对外可用和实际 Vulkan 正确性方面仍有几个重要问题：

1. [已完成] 库外用户无法通过公开 API 获取队列。
2. [已完成] 图像按子资源首次使用时可能漏生成 `UNDEFINED` 布局转换。
3. [已完成] 图像范围缺少溢出、零值和有效范围验证。
4. [待处理] BufferUsage 没有参与命令合法性验证。
5. [待处理] DescriptorSet 资源追踪尚未实现，因此非空 descriptor set 当前无法使用。

## 问题清单

### 1. [已完成] P0/P1：外部用户无法获得队列

原实现只有 backend-facing queue trait，库外没有入口取得并操作队列，因此无法完成 recorder 和 submit 流程。

因此外部 crate 无法完成预期流程：

```rust
let queue = device.queue(QueueKind::Graphics) ?;
let recorder = queue.recorder() ?;
```

完成方案：

- `RhiBackend`、`RhiQueue` 和其他 backend-facing trait 均为 `pub(crate)`；
- 全局 backend 现在使用 `RwLock<Option<Arc<Backend>>>` 保存；`Backend` 内部才持有 `Arc<dyn RhiBackend>`；
- `bootstrap()`、`Backend::global()` 和 `terminate()` 是公开生命周期入口；底层 `backend()` 仅 crate 内可用，用户通过 `Backend` 获取资源与队列接口；
- buffer/image/view/sampler 已采用公开 concrete wrapper；
- `RhiBackend::queue` 返回 `Arc<dyn RhiQueue>`；公开 `Queue` wrapper 直接持有该对象，并转发 recorder、submit 和 force-flush 接口；
- `Recorder` 的公开命令只接收 concrete resource wrapper，不再泄漏 crate-private 的 `Rhi*` trait。

### 2. [已完成] P1：图像不同子资源的首次使用可能缺少 barrier

在 [src/rhi/resource.rs](../src/rhi/resource.rs) 的 `ensure_initial_state` 中，只有状态列表完全为空时才插入初始状态：

```rust
if ! states.is_empty() {
return;
}
```

可能的路径：

1. 第一次使用 mip 0，生成 `UNDEFINED -> TRANSFER_DST`。
2. 后续使用 mip 1。
3. mip 1 与现有状态不重叠，因此没有 `before = None` 的转换。
4. `replace_state` 直接把 mip 1 标记为已使用，但 Vulkan 布局仍是 `UNDEFINED`。

同样的问题也存在于先使用一个小 image view、之后使用覆盖更大范围的 image 操作。

建议让图像 tracker 覆盖完整资源的所有子资源，或者在计划阶段为未覆盖的子范围显式插入初始布局转换。

this is a tracker issue, not cover all situations, we need redesign, talk later

完成情况：tracker 已区分 buffer range 与 image subresource 状态。Image 使用稀疏 `(aspect, mip, layer)` cell；未出现的 cell 代表 `UNDEFINED`，因此每个不同 mip/layer 的首次使用都会生成初始 transition。跨队列 release/acquire 也以相同 cell 范围配对。

### 3. [已完成] P1：图像范围没有完整校验

公开的 `ImageSubresourceRange` 和 `ImageSubresourceLayers` 没有验证以下情况：

- `mip_count == 0`
- `layer_count == 0`
- `base_mip + mip_count` 溢出
- `base_layer + layer_count` 溢出
- 范围超出实际 image 的 mip/layer 数量

`replace_state` 在 [src/rhi/resource.rs](../src/rhi/resource.rs) 中使用普通加法计算结束位置，恶意或错误输入可能在 debug
构建中 panic，在 release 构建中产生错误范围。最终范围还会被传给 Vulkan barrier。

建议增加统一的 image range validator，并在 `image_use`、rendering attachment 和 copy 命令进入 planner 前调用。

#### 修复方案

采用“构造时建立基本合法范围，`Recorder::build()` 再针对具体 image 做边界验证”的方案。RHI 只保证子资源范围安全，不重复实现完整的 Vulkan 规范验证。

完成情况：

- 已加入 `ImageAspectFlags` 和 `ImageSubresourceInfo`；
- 已加入 `RhiImage::subresource_info()` 和 `RhiImageView::image_info()`；
- `ImageSubresourceRange::validate()` 会检查空范围、整数溢出、资源边界和 aspect；
- command analyzer 会在 image copy 和 rendering attachment 进入 planner 前验证范围；
- tracker 已将 buffer 区间状态与 image 状态分离；
- image 使用稀疏 `(aspect, mip, layer)` cell，缺失 cell 明确代表未初始化；
- 已补充非法范围与不同 mip 首次使用的回归测试。

首先让 image 暴露 tracker 所需的后端无关信息：

```rust
bitflags::bitflags! {
    pub struct ImageAspectFlags: u8 {
        const COLOR   = 1 << 0;
        const DEPTH   = 1 << 1;
        const STENCIL = 1 << 2;
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ImageSubresourceInfo {
    pub aspects: ImageAspectFlags,
    pub mip_levels: u32,
    pub array_layers: u32,
}

pub(crate) trait RhiImage: RhiObject {
    fn id(&self) -> ObjectId;
    fn subresource_info(&self) -> ImageSubresourceInfo;
}
```

`RhiImageView` 同样需要暴露底层 image 的子资源信息：

```rust
pub(crate) trait RhiImageView: RhiObject {
    fn image(&self) -> &dyn RhiImage;
    fn subresource_range(&self) -> ImageSubresourceRange;
}
```

为公开 range 提供统一验证，至少检查：

- mip/layer count 不为零；
- `base + count` 使用 `checked_add`，不能溢出；
- mip/layer 结束位置不能超过具体 image；
- image 必须包含请求的 aspect。

```rust
impl ImageSubresourceRange {
    pub fn validate(self, info: ImageSubresourceInfo) -> RhiResult<ValidImageRange>;
}
```

验证成功后转换成 tracker 内部类型：

```rust
struct ValidImageRange {
    aspect: ImageAspect,
    mip_start: u32,
    mip_end: u32,
    layer_start: u32,
    layer_end: u32,
}
```

tracker 的 `overlaps`、`intersection` 和 `replace_state` 只接收 `ValidImageRange`。这样内部不再重复执行未经检查的 `base + count`，也不会因为外部输入发生整数溢出。

验证时机：

1. `ImageSubresourceRange::new()` 检查 count 非零和整数溢出；
2. `create_image_view()` 根据具体 image 检查 aspect、mip 和 layer 边界；
3. `Recorder::build()` 在 `resource.rs::analyze()` 中检查 copy 命令直接引用的 image range；
4. rendering attachment 使用已在 view 创建时验证过的范围，但 build 阶段仍可做防御性检查。

建议将 `ImageSubresourceRange` 字段改为私有并提供构造函数。构造函数只能检查非零和溢出；是否超出资源边界必须等到 range 与具体 image 关联时验证。

本项不负责以下 Vulkan 特有规则：

- format 是否支持某种 usage；
- resolve format 和 sample count 是否匹配；
- 压缩格式的 copy block 对齐；
- Vulkan layout 和 native handle 合法性。

这些规则继续由 Vulkan backend 或 validation layer 处理。

### 4. [待处理] P1/P2：BufferUsage 没有参与命令验证

创建 buffer 时会检查 usage，但 `VkBuffer` 没有保存 usage 信息，Recorder/后端也没有检查实际操作是否满足创建声明。例如：

- 没有 `TRANSFER_SRC` 却作为 copy source；
- 没有 `VERTEX` 却绑定为 vertex buffer；
- 没有 `INDEX` 却绑定为 index buffer；
- 没有 `INDIRECT` 却用于 indirect draw/dispatch。

建议将 usage 保存到 backend resource，并在 `validate_commands` 或资源 planner 中验证。

### 5. [待处理] P1/P2：DescriptorSet 资源追踪尚未实现

在 [src/rhi/resource.rs](../src/rhi/resource.rs) 中，非空 descriptor set 会直接返回：

```text
descriptor resource introspection is not implemented yet
```

这意味着真实 shader 资源绑定目前无法通过 Recorder 构建。后续需要让 descriptor set 暴露其 buffer/image 使用信息，纳入同一套
tracker 和 barrier 计划。

### 6. [已完成] P2：资源 tracker 的 poison 错误会 panic

`TrackerTransaction::commit`、rollback 以及 `ResourceTracker` 的部分公开方法使用 `expect("resource tracker poisoned")`。如果
mutex 被 poison，错误路径会 panic，而不是返回 `RhiError`。

正常运行中不容易触发，但对于库级 API，建议统一改成可恢复的错误处理，或明确记录这是不可恢复的进程级故障。

need a solution

完成情况：

- fallible 的 plan/reserve 路径会把 mutex poison 转换为 `RhiError`；
- native submit 成功后的 commit 与 Drop rollback 属于必须完成的清理路径，遇到 poison 时恢复 inner value，避免已提交工作与追踪状态永久分离；
- RHI `DiscardManager` 同样在清理路径恢复 poisoned mutex，并通过 `RhiBackend::destroy_*` 执行后端销毁；
- 资源状态与生命周期已拆分成 `ResourceTracker` 和 `LifetimeTracker`，原来的 `pending: u32`、`retired`、单一 `last_use` 已从资源状态 tracker 移除；
- lifetime 使用 `Option<ReservationId>`，并按 timeline 保存最新 `SyncPoint`，避免多 queue 使用对象时过早销毁。

### 7. [待处理] P2：Vulkan rendering/copy 参数验证不足

当前后端仍缺少以下校验：

- rendering extent 和 layer count 是否非零；
- resolve attachment 的格式、采样数和范围是否匹配；
- image copy region 是否在 image 有效范围内；
- attachment load/store 组合是否合法；
- image view 的范围与实际 image 描述是否匹配。

这些参数目前大多直接传给 Vulkan，错误通常会在 validation layer 或 GPU 执行阶段才暴露。

this is what validation layer should do, we don't do this

### 8. [待处理] P2：后端能力仍有明确未实现部分

目前 Vulkan 后端还未实现：

- image 创建；
- image view 创建；
- sampler 创建；
- debug label 的实际 Vulkan 调用；
- descriptor 资源 introspection；
- 完整的 image discard/销毁路径。

这些属于功能缺口，不应与 Recorder 生命周期 bug 混淆，但应在 API 稳定前明确标注。

### 9. [待处理] P2：physical device 没有保存在 Device 中

`Device::new` 选择 physical device 并用它创建 logical device 和 allocator，但 `Device` 本身没有保存 `vk::PhysicalDevice`。

这不会影响当前已实现的 buffer/queue 路径，但后续 image 格式查询、memory properties、feature 查询和 debug 工具都需要
physical device。建议把它作为 `Device` 的字段保存。

## 建议实施顺序

1. 暴露可用的 backend/queue API。
2. 修复 image tracker 对未初始化子范围的覆盖和 barrier 生成。
3. 增加 image range 的零值、溢出和资源边界校验。
4. 保存并验证 BufferUsage。
5. 设计 descriptor set 的资源依赖接口。
6. 补充 Vulkan rendering/copy 参数校验。
7. 最后处理 mutex poison、clippy 和其余后端功能缺口。
