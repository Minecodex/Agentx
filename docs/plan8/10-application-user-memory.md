# 应用用户映射与记忆连接

状态：已完成，2026-10-17 验收。本文替换记忆命名空间资源及仅平台登录用户可使用长期记忆的设计。

## 产品与边界

记忆连接是唯一的记忆资源，使用连接名称、Endpoint、健康检查路径、凭证和所属部门管理。删除资源名称、命名空间和访问模式；资源使用授权同时允许召回和写入，不存在只读记忆资源。Credential 依赖和运行身份授权继续逐项校验。

应用部署的高级配置使用可空的 `apiUserIdInput` 选择已发布工作流版本的顶层字符串输入。API Key 仍验证应用调用权限，输入值只作为调用方声明的用户 ID，不进行账号、真实性或归属核验。配置随部署冻结，后续部署沿用上次选择。

平台调用使用登录用户；渠道调用使用验签后渠道账号范围内的发送者；API 调用使用所选输入。来源不同的相同 ID 不合并，同一 App 的不同 API Key 使用相同声明 ID 属于同一 API 用户。身份独立于平台操作人，在入站时写入调用和执行快照，Runtime 不回读 Control。无有效身份时调用仍可执行，但个人长期记忆不可用，不使用共享或运行范围替代。

## 会话与记忆

创建会话接受初始 `input`，固定解析后的用户身份及初始业务参数；后续聊天继承这些参数并叠加本轮消息。响应、查询和执行详情展示用户来源及原始 ID。用户标识用于归属和筛选，不构成新增用户认证。连续对话继续由会话 ID 区分。

个人记忆范围由租户、环境、应用、工作流、稳定节点及来源用户身份自动生成；发布版本、部署 ID、API Key 和会话不参与记忆身份。不同 Agent 节点独立，Studio 使用独立调试域。Mem0 工具与内部记忆节点使用同一范围构造方法，审计和清理消费相同的范围。

## 身份、协议和查询

`endUser` 保存 `source`（`platform`、`channel`、`api_parameter`）、`sourceId` 和原始 `externalUserId`。渠道来源使用 Adapter 已验签的 `providerConnectionId` 与发送者 ID，缺少发送者或占位 `unknown` 不生成个人身份。API 参数没有用户认证语义；换 Key 不换用户，同一 Key 可以传入不同业务用户。执行的 `initiator_user_id` 继续只表示平台操作人。

会话创建示例：

```json
{"input":{"customerId":"customer-42","region":"Shanghai"},"title":"业务咨询"}
```

部署选择 `apiUserIdInput=customerId`。后续 `/sessions/{id}/messages` 只传本轮 `parts`，Runtime 用初始业务参数叠加消息映射，再做完整输入 Schema 校验。聊天映射可以保留需在会话创建时提供的必填业务字段。调试台创建会话时用统一 Schema 表单填写初始业务参数。删除旧的 `externalUserId` 请求字段，响应仍展示解析结果。未取得身份时 Invocation 返回 `userIdentityDiagnostic=MEMORY_USER_ID_UNAVAILABLE`，不新增用户认证错误。

应用内“调用记录”和执行详情展示来源与原始 ID；会话、调用记录及执行查询支持 `externalUserId`、`userSource` 筛选，ID 按原始字符串精确匹配。应用调用记录使用 `/applications/{application}/invocations` 的快照游标分页；用户筛选不增加外部用户认证，也不改变原有查询权限。

Mem0 `user_id` 由同一个 `MemoryScopeV1` 构造，为 `agentx:sha256:<digest>`。运行任务中的节点 key 通过冻结 IR 解析为节点 ID；Agent 会话使用稳定节点 ID。记忆审计和清理使用相同 scope hash 加连接 ID，不再按连接版本隔离。清理保留现有墓碑边界，阻止已清理范围被旧 Worker 再次写入。子工作流复制来源用户及环境快照，并保存真正的工作流 ID，避免以版本 ID 切断记忆。

## 实施与验收

直接修改初始化 DDL、资源引用和协议，删除命名空间旧接口，不提供历史迁移、兼容包装或备用身份入口。更新生成契约、前端类型、Fixture 与文档。复用现有依赖、统一 UI 和 Python/Playwright 基础设施。

验证连接 CRUD/授权/删除、部署配置、三来源身份、会话参数继承及用户筛选、无身份诊断、恢复与子工作流、真实 Mem0 跨会话和跨版本召回、用户/节点/来源隔离、API Key 轮换。系统测试统一通过 `pytest tests/e2e` 在临时 Kubernetes 命名空间运行，证据写入 `.local/artifacts`，最终清理并恢复开发服务。

## 验收结果

- `cargo xtask check` 完整通过：596 项 Rust 测试、178 项验收测试、486 项前端测试，以及 Clippy、格式、边界、文件行数、桌面测试类型检查、前端构建和插件检查。两项需要真实企业渠道账号凭证的 Rust 探针未执行；渠道身份使用经过 Adapter 验签的钉钉回调验证。
- 真实 Mem0 场景在 `user-memory-20261016-a3` 通过：跨会话及跨版本召回、同用户跨 API Key 共享，不同用户、来源和稳定节点隔离，平台及渠道自动识别，无身份时不发出个人记忆请求。
- 会话、桌面及连接场景在 `user-memory-20261016-a4` 通过：无平台账号的原始用户字符串、初始业务参数继承、身份冻结、参数未设置或缺失的诊断、会话/执行/调用筛选、部署参数保存和预填、旧字段消失、连接 CRUD、固定使用授权及连接状态编辑。
- a3 的桌面断言因历史部署也显示相同参数而定位到多个元素，已限定到最新部署并在 a4 重跑通过；保留此前环境、模型流中断和测试等待问题的证据，未放宽业务断言。
- 11 个最终候选镜像的源码哈希均为 `615d598ec21b2c43e57d84494233d5aeb72d1e510b89acaf482fbba94d972e0e`。全部临时命名空间已删除，开发环境 12 个 Deployment 恢复为 `1/1`。

汇总：`.local/artifacts/user-memory-identity/verification.json`。完整检查日志：同目录 `xtask-check.log`；真实 Mem0 与桌面证据分别位于 `.local/artifacts/e2e/user-memory-20261016-a3`、`.local/artifacts/e2e/user-memory-20261016-a4` 和 `.local/artifacts/playwright/local/user-memory-20261016-a4`。

## 本地发布

2026-10-17 已重新构建并发布 `local-user-memory-20261017`，入口为 `https://agentx.localhost`。11 个镜像的源码身份与已验收工作树一致；四个 Helm Release 已更新，19 个工作负载就绪，8 个运行中的产品 Pod 已逐一核对镜像摘要。Doctor、HTTPS 及静态资源、新初始化 DDL 和 Runtime 用户投影检查通过。

Control 和 Runtime 分别切换至 `agentx_control_memory_20261017`、`agentx_runtime_memory_20261017`。50 张配置表的 256 行账号、模型、凭证及资源配置按字节校验后保留；工作流、应用和执行历史未转换，仍留在原数据库。新库不包含旧记忆命名空间实体。初始化的一次性 Job 已清理。

本地发布配置与验证记录：`.local/artifacts/local-user-memory-20261017/production-values.yaml`、同目录 `verification.json`。
