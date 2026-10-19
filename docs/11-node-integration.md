# 画布节点与插件接入

本文描述 plan6 完成后的唯一节点接入模型。Workflow Definition 8.0、Node Manifest 3.0、Canvas Plugin Package Protocol 1、Plugin SDK API 2 和 Runner RPC 2 均按精确版本拒绝未知输入，不保留旧 Remote Action 或旧 Manifest 入口。

## 1. 节点分类

| 类别 | 定义/UI | 执行 |
|---|---|---|
| 导入画布插件 | 不可变 ZIP 中的 Manifest、React ESM/CSS | Worker 内 Node.js Runner |
| `agentx/data` | 统一注册入口；Set/List 保留专用 Studio 面板 | 公开 TypeScript SDK + Node Runner |
| `agentx/http` | 统一注册入口；HTTP 保留专用 Studio 面板 | TypeScript SDK 调用 `ctx.http`，Rust 托管出口、凭据、账本和 Artifact |
| If/Merge/Loop/Approval/Sub-workflow/Model/Agent/Code | `src/plugins/builtin/core` 包 Manifest 与统一前端包注册 | Rust 原生状态机、Provider 或 Sandbox 能力 |
| Start/Exit | Studio 边界组件 | Rust Boundary 语义，不创建伪 Node/Attempt |

MCP、Skill、Knowledge、Memory、Credential 和 Sandbox Profile 是绑定到节点的资源，不作为第二套独立画布节点目录。

## 2. 插件包

管理员从“资源 → 画布插件”导入 `.agentx-plugin` ZIP。Control 校验安全路径、文件数、压缩后与展开后预算、UTF-8、协议/SDK版本、包 ID、SemVer、自包含 ESM、节点身份、Manifest、Trace renderer Schema 和摘要；预览确认后以事务安装。

包身份由 `packageId + packageVersion + sha256 ZIP digest` 决定。同版本同摘要重复确认幂等，同版本不同摘要冲突。同一 Workflow 中同一包只允许一个版本；Node IR 冻结 `packageId/packageVersion/bundleDigest/runtimeArtifact/uiSource/uiStyles/traceRenderers`。Control 把 Runtime Artifact 作为不可变对象投递到 Runtime OSS，Worker 校验本地缓存，缓存损坏时只按同一对象摘要重新读取，不回查 Control Catalog 或内联执行源码。

更新默认版本只影响之后添加的节点。停用版本后 Catalog 不再允许新使用，Draft 可以保留并修复已有节点，Debug、发布、Rollback/重新激活会拒绝停用版本。已激活 Deployment 和已经创建的 Attempt 继续使用冻结 Bundle。Draft、Workflow Version、Deployment 和执行制品引用保护删除；未引用版本删除时同步清理对象。

`agentx/*` 包 ID 保留给内置包。内置 Set/List/HTTP 与导入插件共用 `plugin_nodejs` 执行协议、Worker 有界调用进程和结果校验，但在管理页只读展示并随 Agentx 发布。

## 3. UI SDK

Web 以 bundle digest 缓存并动态导入编译后的 `data:` ESM。插件只能使用宿主提供的 React 和公共组件，不得打包第二份 React或包含未解析 import。扩展点包括：

- `Panel`：配置参数、字段错误、只读状态、Provider Options、Reference Catalog、资源和 `updateParameters`。
- `Canvas`：节点卡片中的紧凑内容。
- `Result`：Node Inspector 的只读结果。
- `traceRenderers`：版本化 `plugin_content` 的只读业务视图。

宿主提供 `Field/Input/Button/Select/SmartInput`，维持变量选择、Schema过滤、资源授权、自动保存和 Undo/Redo。CSS按摘要引用计数并在最后一个挂载点卸载；模块、创建函数和渲染异常由 Error Boundary 隔离。Trace 始终保留标准 JSON 展示。

全部内置节点面板通过 `node-panel-registry.tsx` 解析；旧 `ACTION_PANELS` 固定分支已删除。插件版本切换显示包版本和摘要，保留当前参数并让服务端权威校验暴露需要修复的端口、配置和引用错误，不运行 migration 或自动改写。

## 4. 设计时调用

`POST /api/v1/node-definitions/{nodeType}/versions/{version}/resolve` 和 Provider 端点由 Control 鉴权，再使用短期 Service JWT 调用 Runtime Internal API：

`POST /internal/runtime/v1/plugin-design-operations:execute`

Runtime Gateway 使用独立有界容量启动 Runner，执行 `node.resolveDefinition` 或 `node.invokeProvider`。Control 不启动 Node。输入和输出是纯 JSON；未知方法、协议/API不匹配、超时和非法结果明确失败。Provider 的第二个参数是带 AbortSignal 的设计时宿主 Context，可访问节点已选择资源的冻结快照，并通过 Runtime 的只读 HTTP、Model、Credential descriptor 和临时 Artifact 桥接完成字段发现。静态插件没有导出解析函数时返回 Manifest 的固定端口和输出 Schema。

设计时调用固定为十秒内的同步交互，使用独立两槽并发预算；deadline 或显式取消会终止所属进程树。它不创建 Workflow Execution 或 Attempt，也不进入业务 Redis Queue。纯契约解析和只读 Provider 没有需要恢复的业务终态；正式 `node.execute` 仍通过 Worker Claim、Lease、Fencing 和恢复运行。

## 5. Runner 与宿主能力

Worker 镜像固定 Node.js 24.20.0 和仓库 Runner。Tokio 为每次调用创建独立 Node 进程，调用终止后回收整个进程树和临时目录；按摘要保留源码缓存，不复用 ESM 模块环境。`pluginMaxProcesses` 同时控制插件消费循环和进程并发，满载按 Attempt deadline 等待。Linux 创建独立进程组，Windows 使用 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` Job Object；超时、取消、Lease丢失、Worker drain和 Future drop都会终止整个进程树。

stdin/stdout 使用换行分帧的 JSON-RPC 2.0。启动必须完成 `runner.initialize`，Runner 协议与 SDK API 均为 2。业务方法是 `node.execute`；Runner 反向调用：

| 方法 | Rust 宿主责任 |
|---|---|
| `host.http` | Egress、Credential注入、Runtime Call账本、幂等、结果未知、二进制 Artifact |
| `host.model` | 冻结 Model资源、Provider格式、计量与输出规范化 |
| `host.credentials.list` | 只返回非敏感描述，不返回 Secret |
| `host.artifacts.read/put` | 调用目录中的授权文件流、相对路径/大小/Hash校验、Runtime对象和Execution/Node引用 |

参数在进入 Runner 前已生成公共值和逐 Item值；插件不执行另一套表达式。结果只能是 `completed` 或 `failed`，输出端口、Item、Cardinality、Schema和 Artifact引用仍由 Runtime按冻结 IR校验。外部写调用需要逻辑幂等键；Provider结果未知通过专用 RPC错误传播为 `OutcomeUnknown`，不会盲目重试。

子 Workflow 的每个固定版本由 Control 使用该版本自己的解析后 Manifest 编译成不可变 IR 对象。Runtime 校验发布签名、闭包对象引用、对象大小与摘要后直接使用该 IR；它不会用 Runtime 当前内置 Registry 重新编译，因为当前 Registry 不包含发布时插件的动态端口契约。

## 6. Trace

平台自动生成 Execution/Node/Attempt Span和输入输出。Runner通过 `trace.event` 实时发送 `plugin_operation` started/updated/finished；AsyncLocalStorage保持嵌套/并发 Promise父子关系。宿主 HTTP/Model Runtime Call挂在当前插件 Span下并保留唯一 `runtimeCallId` 和平台计量。

业务内容使用固定 `plugin_content`，由 `packageId/packageVersion/bundleDigest/nodeType/typeVersion/contentType/contentVersion/label/data` 选择冻结 renderer。Runtime按包声明 JSON Schema检查内容，超预算或非法内容只生成诊断警告，不修改业务终态。Trace继续走 MySQL Outbox、Redis、Observability和ClickHouse；历史 renderer按执行摘要读取，停用和当前默认版本不会改变历史展示。

## 7. 开发与验证

管理页可下载自包含模板。根 `AGENTS.md`、字段级文档、vendored 类型和 Runner允许在 Agentx仓库之外运行：

```bash
pnpm install --frozen-lockfile
pnpm check
pnpm test
pnpm build
pnpm pack:plugin
```

模板的 `test` 使用正式 Runner执行编译产物，`pack:plugin`固定 ZIP时间、权限和顺序。最终验收仍必须通过页面真实上传、画布操作、保存重开、Debug/发布、下游引用和 Trace，并由 `pytest tests/e2e` 在临时 Kubernetes Namespace 编排。

契约生成与漂移检查：

```powershell
cargo run -p agentx-runtime-contracts --bin generate-contracts -- contracts/schemas/runtime-v1 contracts/openapi/runtime-internal-v1.json contracts/openapi/observability-internal-v1.json
cargo run -p agentx-runtime --bin generate-studio-catalog -- src/web/src/features/workflow-designer/testing/studio-catalog.fixture.json
cargo xtask check
```

画布的整图动态契约使用 `/api/v1/workflows/{id}/draft/resolve-plugins`，复用保存时依赖解析顺序；单节点 resolve 继续作为 SDK 设计时工具。SDK 2 文件协议与 Trace 预算降级见 `plan6/contracts.md`。

## 工作流输入交互

中间回复和输入等待统一归入 `agentx/core`，持久化与恢复由 Runtime 承担。`input_waits`、`workflow_messages` 和 `channel_input_receipts` 分别保存等待、消息和渠道幂等凭据；Hook 复用 delivery outbox。平台入口共享等待节点发现及回复序列编辑器，定时和测试用例版本冻结 `waitInputs`，评测投影保存实际等待和消息。详见 [工作流交互协议](14-workflow-interactions.md)。
