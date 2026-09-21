# Queue / Recorder 生命周期规范

## 1. 目标

本设计定义 queue、recorder、command buffer、资源状态 tracker 和 lifetime tracker 之间的职责与生命周期。

核心目标：

- queue 是 backend-neutral trait；具体后端 queue 拥有自己的 timeline、command pool、空闲 command buffer 和待完成 command buffer。
- recorder 在创建时取得并独占一个 `Arc<dyn RhiCommandBuffer>`。
- recorder 按输入顺序保存 `Command<'a>`，在 `build()` 时通过全局 tracker 自动插入 barrier，并录制到后端 command buffer。
- `Recorder::build(self)` 成功后产生 `BuiltRecorder`；queue 通过 `queue.submit(built_recorder)` 提交，成功后 command buffer 进入该 queue 的 pending 列表。
- queue 定期读取自己的 timeline，把已经完成的 pending command buffer 移回 unused 列表。
- command buffer 回收不进入资源生命周期追踪或 discard manager。
- RHI 层提供设备级全局 `LifetimeTracker` 和 `DiscardManager`；manager 判断销毁时机，native destroy 仍由 backend 实现。

本文描述行为契约，不固定最终 Rust 类型名和内部数据结构。

## 2. 所有权边界

### 2.1 Device / backend

公开 `Backend` wrapper 由全局 singleton 保存；它内部持有 crate-private `Arc<dyn RhiBackend>`。用户只通过 `Backend` 获取资源和同步接口，backend trait 不进入公开 API。

设备拥有：

- 全局 `ResourceTracker`。
- 全局 `LifetimeTracker`。
- 全局 `DiscardManager`。
- graphics、transfer 等后端 queue。

`ResourceTracker` 和 `LifetimeTracker` 都不属于任何一个 queue。queue 分别持有它们的共享引用，例如
`Arc<ResourceTracker>` 和 `Arc<LifetimeTracker>`。两者不能合并：前者只负责 buffer/image 的 barrier 状态，后者负责所有可能被 GPU command 引用的对象生命周期。

资源延迟销毁与 command buffer 回收是两个独立系统：

- `LifetimeTracker` 判断对象何时不再被 recorder 或 GPU 使用，`DiscardManager` 保存 backend object，backend 销毁自己的 native 对象。
- queue-local command pool 复用录制与提交所需的 command buffer。

### 2.2 Queue

`Queue` 是 backend-neutral trait。调用者能看到 queue 行为，但看不到 pool、unused 和 pending 等后端字段。

每个具体后端 queue 实现独占：

```text
BackendQueue
├── native queue
├── timeline / fence / shared event
├── next timeline value
├── backend command pool state
├── unused: Vec<Arc<dyn RhiCommandBuffer>>
├── pending: Vec<PendingCommandBuffer>
└── shared ResourceTracker reference
```

其中 command pool 不是公共 RHI trait 的一部分。它只存在于具体后端实现中：

- Vulkan：`VkCommandPool` 以及由它分配的 `VkCommandBuffer`。
- D3D12：command allocator/list 的后端组合与复用状态。
- Metal：`MTLCommandQueue` 以及后端可复用的录制辅助状态。经典 `MTLCommandBuffer` 提交后不可复用；Metal 4 的 `MTL4CommandBuffer` 则允许重新开始、编码并再次提交。

pool 不是独立 trait，也不进入 `Queue` trait。它只存在于 Vulkan、D3D12、Metal 等具体 queue 实现内部。

queue 是 command buffer 生命周期的唯一 owner。discard manager 不保存 command buffer。

### 2.3 Recorder

recorder 由指定 queue 创建，并拥有：

```text
Recorder<'queue, 'command>
├── originating queue exclusive lease
├── command_buffer: Arc<dyn RhiCommandBuffer>
└── commands: Vec<Command<'command>>

BuiltRecorder<'queue>
├── originating queue exclusive lease
├── command_buffer: Arc<dyn RhiCommandBuffer>
├── resource state transaction
└── lifetime reservation
```

recorder 不拥有 tracker，也不拥有 timeline。

`Recorder` 只能录制和 build，不能提交；只有 `BuiltRecorder` 能提交。built recorder 只能提交回创建它的 queue，不能跨 queue 提交。

## 3. RHI command buffer

`unused` 和 `pending` 直接保存 `Arc<dyn RhiCommandBuffer>`。这些字段是具体后端 queue 的私有实现细节，不对用户暴露。

`RhiCommandBuffer` 是可复用的 RHI wrapper，不要求其内部原生对象在所有后端上具有相同的复用规则：

后端约束：

- Vulkan wrapper 可以持有并 reset `VkCommandBuffer`。
- D3D12 wrapper 可以持有 allocator 和 command list；必须等 GPU 完成后才能 reset allocator。
- 经典 Metal wrapper 被重新取用时创建新的 `MTLCommandBuffer`；Metal 4 wrapper 可以复用 `MTL4CommandBuffer`。

`Arc` 在 unused、pending 和 recorder 之间只移动、不克隆。command buffer 位于 recorder 时必须是唯一引用，并且不得存在 `Weak` 引用，从而可以通过 `Arc::get_mut` 或等价机制取得 `&mut dyn RhiCommandBuffer` 进行录制。

因此，`Arc<dyn RhiCommandBuffer>` 表示可再次准备和录制的 RHI 对象，不等同于所有后端都能原样复用的 native handle。

## 4. Queue 状态

### 4.1 Unused

`unused` 保存当前没有被 recorder 占用、没有被 GPU 使用、可以安全 reset/重新初始化的 `Arc<dyn RhiCommandBuffer>`。

从 `unused` 取出的 command buffer 必须先由具体 queue 实现准备为初始录制状态。准备操作可能包括：

- reset Vulkan command buffer；
- reset D3D12 command allocator/list；
- 为经典 Metal wrapper 创建新的 `MTLCommandBuffer`，或 reset Metal 4 wrapper。

### 4.2 Pending

`pending` 保存已经成功提交但尚未确认完成的 command buffer：

```text
PendingCommandBuffer {
    command_buffer: Arc<dyn RhiCommandBuffer>,
    completion_value: u64,
}
```

同一个 queue 的 timeline 值必须单调递增。pending 可以保持提交顺序；如果 timeline 保证按顺序完成，flush 可以从队首连续回收。

### 4.3 Timeline

timeline 由 queue 自己拥有。一次成功提交：

1. 取得新的 timeline value。
2. 将 command buffer 和必要的跨 queue waits 一起提交。
3. 在本 queue timeline 上 signal 新 value。
4. 返回对应 `SyncPoint`。
5. 将 command buffer 和该 value 放入 pending。

提交失败不得推进对外可见的 tracker 状态，也不得把 command buffer 放入 pending。

## 5. Queue 操作

以下接口表示语义，不固定为最终 trait 签名：

```text
make_recorder() -> Result<Recorder<'queue, 'command>>
allocate() -> Result<Arc<dyn RhiCommandBuffer>>
submit(BuiltRecorder<'queue>) -> Result<SyncPoint>
force_flush() -> Result<()>
```

### 5.1 `allocate()`

`allocate()` 的职责：

1. 优先从 `unused` 取一个 command buffer。
2. 如果 unused 为空，先执行一次非阻塞 flush，并再次尝试从 unused 取值。
3. 如果仍然为空，则从本 queue 的后端 pool 创建新的 `Arc<dyn RhiCommandBuffer>`。
4. 让具体后端把 command buffer 准备为可录制状态。
5. 返回 command buffer，所有权转移给 recorder。

`allocate()` 不访问 discard manager。

### 5.2 非阻塞 flush

queue 提交时必须尝试一次非阻塞 flush：

1. 查询 queue timeline 当前 completed value。
2. 从 pending 中取出 `completion_value <= completed` 的 command buffer。
3. 将这些 command buffer 移入 unused。
4. 未完成的 command buffer 保留在 pending。

该操作不等待 GPU，也不销毁 command buffer。

`submit()` 每次都执行该 flush；`allocate()` 在 unused 为空时也执行该 flush，避免 GPU 已完成但尚未发生下一次 submit 时产生额外分配。

### 5.3 `submit()`

`submit()` 的类型签名只接受 `BuiltRecorder`。运行时仍校验它由当前 queue 创建且尚未提交。

- 由当前 queue 创建；
- 已由 `build()` 成功产生；
- 尚未提交；

的 built recorder。

建议顺序：

1. 执行非阻塞 flush，把已经完成的 pending command buffer 移入 unused。
2. 从 built recorder 取出 `Arc<dyn RhiCommandBuffer>`、tracker transaction 和 wait 列表。
3. 分配本次 signal value。
4. 后端执行 native submit。
5. native submit 成功后，用返回的 `SyncPoint` commit tracker transaction。
6. 把 command buffer 放入 pending。
7. 返回 `SyncPoint`。

失败行为：

- native submit 失败时，tracker transaction 必须 rollback。
- command buffer 没有进入 GPU pending 状态，应回到 unused；下次取用时由后端 reset/reinitialize。
- 调用者得到错误；built recorder 已被消费，不能再次提交。

### 5.4 `force_flush()`

`force_flush()` 的语义等同于该 queue 的 `wait_idle`，不是整个 device 的 `wait_idle`：

1. 等待该 queue 已提交的全部工作完成。
2. 将所有 pending command buffer 移入 unused。
3. pending 变为空。
4. 不销毁 pool、timeline 或 command buffer。

如果后端没有直接 queue-idle 操作，可以等待该 queue 最后 signal 的 timeline value 来实现。

queue drop 前必须执行等价的等待，然后才能销毁 timeline、command buffer 和后端 command pool。

## 6. Recorder 操作

### 6.1 创建

`queue.make_recorder()`：

1. 取得本 queue 的独占 lease；build 成功时 lease 转移给 built recorder，并持续到 submit 或 drop。
2. 调用本 queue 的 `allocate()`。
3. 创建 recorder，并把 `Arc<dyn RhiCommandBuffer>` 所有权移入 recorder。
4. 初始化空的 command vector。
5. recorder 进入 recording 状态。

创建 recorder 本身不开始改变全局 tracker 状态。

同一个 queue 同时只能存在一个 recorder。其他 recorder 创建、submit 或 `force_flush()` 操作必须等待当前 lease 释放。

### 6.2 添加命令

recorder 按调用顺序保存 `Command<'command>`。动态生成命令可使用：

```text
push(command)
extend(commands)
```

也可以使用消费 `self`、返回 `Self` 的 typed fluent 方法链。每个 `Command` variant 都有对应方法，例如：

```text
queue.recorder()?
    .begin_debug_label("upload")
    .copy_buffer(src, 0, dst, 0, size)
    .release_buffers(buffers, QueueKind::Graphics)
    .end_debug_label()
    .build()?
```

fluent API 只描述 RHI 命令，不公开 native command-buffer 的 begin/finish。

manual release 是命令流中的命令，并保留精确位置，例如：

```text
copy A -> B
release B to graphics
copy C -> D
```

release 不能被移动到 command stream 末尾。

### 6.3 `build(self) -> Result<BuiltRecorder<'queue>>`

`build()` 消费 `Recorder<'queue, 'command>`。成功后返回不再携带 `'command` 生命周期的 `BuiltRecorder<'queue>`，因此命令参数借用只需持续到 build 完成；queue lease、command buffer 和 tracker transaction 被转移到 built recorder。

build 流程：

1. 后端验证对象归属和 queue capability，不允许 foreign backend 对象或 queue 不支持的命令进入 native recording。
2. 验证 command stream 的结构，例如 rendering scope 和 debug label 配对。
3. 请求全局 `LifetimeTracker` 独占预留命令引用的对象。
4. 请求全局 `ResourceTracker` 分析资源读写、范围和 queue ownership，并创建暂定 transaction。
5. state tracker 按原始命令顺序生成 planned stream：
   - 自动 memory barrier；
   - 自动 acquire barrier；
   - manual release command 对应的 release barrier；
   - 原始 native commands；
   - 需要等待的其他 queue `SyncPoint`。
6. 确认 recorder 持有 command buffer `Arc` 的唯一引用，并取得可变 command buffer。
7. 后端开始录制 command buffer。
8. 按 planned stream 录制 barrier 和普通 command。
9. 结束后端录制。
10. 丢弃 command vector，将两个暂定对象、lease 和 command buffer 移入 `BuiltRecorder`。

build 成功不 commit tracker。只有 native submit 成功后才能 commit。

build 失败：

- tracker transaction rollback；
- 不产生 `BuiltRecorder`，因此类型上不可提交；
- command buffer 返回该 queue 的 unused；
- 已部分录制的后端 command buffer 在下次使用前 reset。

因为 build 消费 `self`，类型系统不允许重复 build，也不允许 build 后继续追加命令。

### 6.4 Drop

recording recorder 在 build 前被 drop，或 built recorder 在 submit 前被 drop 时：

- built recorder 的 tracker transaction rollback；recording recorder 尚无 transaction；
- command buffer 返回 originating queue 的 unused；
- command buffer 不进入 pending；
- 不访问 discard manager。
- 释放 queue 独占 lease。

这要求 recorder 持有 queue 独占 lease，保证 recorder 生命周期内 queue 不会先被销毁，并禁止同一 queue 创建第二个 recorder。

## 7. 两类 tracker 的职责

两类 tracker 都是 device-global，因为对象可能跨 queue 使用。queue 只持有共享引用。

### 7.1 ResourceTracker

`ResourceTracker` 只追踪影响 barrier 的状态：buffer range 的 access/queue ownership，以及 image subresource 的 access/layout/queue ownership。

Image view 不拥有独立 barrier 状态。涉及 view 的命令会规范化成其底层 image 与 view subresource range，再由 image 状态生成 barrier。

### 7.2 LifetimeTracker

`LifetimeTracker` 独立追踪所有可能被 GPU command 引用的 RHI 对象：buffer、image、image view、sampler、pipeline 和 descriptor set。每个对象使用统一的 `ObjectId`；`ResourceId` 不再存在。

image view 与底层 image 是两个不同对象。具体 view 持有底层 image 的 `Arc`，命令引用 view 时 lifetime reservation 同时包含 view 和 image：销毁 view 不影响 image 的 barrier identity，而 view 存活也保证 image 不会先被释放。

每个 lifetime entry 包含：

```text
reservation: Option<ReservationId>
last_uses: Map<TimelineId, SyncPoint>
retired: bool
```

`Option<ReservationId>` 表示 build 到 submit/drop 之间的独占预留，不再用计数器表达。`last_uses` 必须按 timeline 保存每条队列的最新值；单个 `SyncPoint` 无法覆盖对象先后被多个 queue 使用的情况。

对象只有同时满足以下条件才能被 backend 销毁：

- 已标记 retired；
- 不存在正在 build/等待 submit 的 reservation；
- `last_uses` 中每个 timeline 的值都已完成。

### 7.3 两阶段提交

tracker 必须区分三个时刻：

1. **plan/reserve**：build 时生成 transitions，暂时预留涉及的资源。
2. **commit**：native submit 成功后，state transaction 写入新的 scheduled state，lifetime reservation 把本次 `SyncPoint` 写入对应 timeline。
3. **rollback**：build、submit 失败或未提交 recorder 被 drop 时，释放预留且不改变 committed state。

资源的 committed state 与对象的 `last_uses` 都表示已成功安排到 GPU timeline，不要求 GPU 已经完成。native submit 失败时，两者都必须 rollback。

跨 queue 使用：

- source recorder 中的 manual release 在指定位置生成 release transition。
- target queue 第一次使用该资源时生成 acquire transition。
- target submit waits 必须包含 source queue 对应的 `SyncPoint`。

## 8. Discard manager 与资源 wrapper

`lifetime.rs` 定义 device-global、mutex-protected 的 `DiscardManager`。它保存明确类型的 `DiscardedObject` 枚举：buffer、image、image view、sampler、pipeline 和 descriptor set；不使用泛型 payload。

公开资源是具体 wrapper，例如 `Buffer { inner: Arc<dyn RhiBuffer>, ... }`。`RhiBackend::create_*` 仍只返回 backend object 的 `Arc<dyn Rhi*>`，不会构造或返回具体 wrapper；`Buffer::new`、`Image::new`、`Sampler::new` 和 `ImageView::new` 才是公开构造入口。

`Backend` 是公开句柄。资源的 `create_*` 方法仅供 crate 内部的 wrapper 构造使用；用户通过 `Backend::global()` 获取句柄，再调用 `Backend::queue(QueueKind)` 获取公开的 `Queue` 句柄。`RhiBackend::queue` 和其他 backend trait 接口保持 crate-private。

`RhiBackend::queue` 返回共享的 `Arc<dyn RhiQueue>`，`Queue` wrapper 直接保存该对象。backend 实现同样以 `Arc` 持有其队列。原先分离的 `RhiQueue`/`QueueBackend` 已合并成单个 crate-private trait；公开的 recorder、submit 和 flush 操作只由 `Queue` wrapper 暴露。

wrapper 实现并转发内部 `Rhi*` 接口。wrapper drop 时把唯一持有的 inner `Arc` 移入 discard manager；backend 生命周期由全局 backend 管理，wrapper 本身不保存 backend 引用。`RhiBackend`、`RhiType` 及其他 backend-facing trait 均为 `pub(crate)`，不会成为用户 API。

`terminate()` 是 backend 资源销毁边界：调用方必须先释放所有 concrete wrapper，使其 inner object 进入 singleton discard manager；`terminate()` 在移除 backend 全局引用前清空 discard manager，并通过 backend 的 `destroy_*` 接口销毁所有对象。

inner 使用 `Arc` 而不是 `Box`，因为 image view 等对象需要共享底层资源。flush 除了检查 timeline 和 reservation，还要求 inner `Arc` 只剩 manager 持有；因此 image 会等引用它的 view 先销毁。

flush 取出安全对象后，根据枚举 variant 调用 `RhiBackend::destroy_buffer`、`destroy_image`、`destroy_image_view` 或 `destroy_sampler`。manager 自身不包含 Vulkan、D3D12 或 Metal native destroy 逻辑。

discard manager 不负责：

- command buffer；
- command pool；
- queue timeline；
- pending/unused command buffer；
- recorder rollback。

这些对象全部由 originating queue 管理。

## 9. 并发模型

同一个 queue 不允许同时存在多个 recorder。lease 从 `Recorder` 创建开始持有，build 成功时转移给 `BuiltRecorder`，直到以下任一事件：

- `queue.submit(built_recorder)` 消费 built recorder；
- recording recorder 或 built recorder 被 drop；
- build 失败，消费中的 recorder 自动完成清理。

allocate、build、submit、flush、timeline value 分配与 command buffer 回收都受该独占模型串行化。`queue.submit(built_recorder)` 必须复用其已持有的 lease，不能再次锁定同一 queue。

不同 queue 可以并行录制和提交，但共享 tracker 的 plan/commit/rollback 必须保持线程安全。

## 10. 必须保持的不变量

- 一个 `Arc<dyn RhiCommandBuffer>` 在任意时刻只属于 unused、pending 或一个 recorder，三者互斥。
- command buffer `Arc` 在 queue 生命周期内只移动、不克隆，并且不创建 `Weak` 引用。
- pending command buffer 在 timeline 完成前绝不能 reset 或重录。
- 同一个 queue 同时最多存在一个 recorder。
- 只有成功的 native submit 才能 commit tracker transaction。
- lifetime reservation 与 state transaction 必须由同一次 native submit 一起 commit 或一起 rollback。
- image view 的 lifetime reservation 必须同时包含 view 和底层 image。
- retired 对象必须等待所有相关 timeline 完成后才能 native destroy。
- manual release 的 barrier 位置与 command stream 中 release command 的位置一致。
- 只有 `BuiltRecorder` 能提交，且只能提交到创建它的 queue。
- queue drop 时不存在 GPU 仍在使用的 command buffer。
- command buffer 永远不进入 discard manager。
- `force_flush()` 回收但不销毁 command buffer。

## 11. 已确认的接口决策

1. `Queue` 是 trait；pool、unused 和 pending 是具体后端 queue 的私有字段。
2. unused 与 pending 直接保存 `Arc<dyn RhiCommandBuffer>`。
3. 提交入口固定为 `queue.submit(built_recorder)`。
4. `submit()` 每次执行非阻塞 flush；`allocate()` 在 unused 为空时也先执行非阻塞 flush。
5. 同一个 queue 不允许多个 recorder 并存；recorder 持有 queue 独占 lease。

## 12. 参考

- [Apple：经典 Metal command buffer 提交后不可复用](https://developer.apple.com/documentation/Metal/setting-up-a-command-structure)
- [Apple：Metal 4 command buffer 可以重复录制和提交](https://developer.apple.com/documentation/metal/understanding-the-metal-4-core-api)
