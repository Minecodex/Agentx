# plan7 本地真实服务验收（2026-10-08）

按用户授权，以既有 Kimi Coding `k3`、小型 CPU Embedding、真实 LightRAG、Mem0 和 OpenSandbox 完成第一批功能测试。首批 14 个 pytest 场景均有通过证据，覆盖关键桌面流程的 9 个 Playwright 用例；修复过程中的失败运行保留。该结论来自专项复测，不等同于 plan7 全量生产认证。

图片、音频及其他模型暂缓；真实飞书、钉钉、企微回复由用户测试。本机门户的宿主机 80/443 仍待 Docker 系统权限确认，不能写成入口已恢复。

## 环境与范围

- Docker Desktop Kubernetes，ARM64，约 10 CPU / 24 GiB。正式 `agentx-prod-*` 业务和既有密钥保留；测试运行在独立 Namespace。
- 文本模型：`https://api.kimi.com/coding/v1`，上游 `k3`。凭证从既有 Vault 读取，仅在内存和临时 Kubernetes Secret 中使用；证据不含 Key。
- Embedding：固定 revision、权重 SHA-256 的 `BAAI/bge-small-zh-v1.5`，512 维，Infinity OpenAI API，真实 Torch CPU 推理、两条线程。
- LightRAG：真实 Kimi 提取实体关系，CPU 模型生成向量，独立 Workspace/PVC。Mem0 `v2.0.15`：真实 Kimi、CPU Embedding、pgvector，启用认证，业务连接使用 Bearer JWT。
- OpenSandbox Server `0.2.2`、Code Interpreter `v1.1.0`，Docker bridge/runc；功能测试使用明确的 local Profile、`secureAccess=false`。正式 production Profile 的 Kubernetes Runtime、强隔离 RuntimeClass、`secureAccess=true` 尚未在本机闭环。
- 系统 E2E 唯一入口为 `pytest tests/e2e`；Python 编排环境与清理，业务浏览器沿用 TypeScript Playwright。构建和核心部署复用 Rust `cargo xtask images`、`agentxctl`。

首批测试资源的价格明确设为 0，验证 Token 数据及零价格显示，当时未验证非零价格的计费准确性。2026-10-09 已继续补齐非零费用专项，见 [第三批自动化验收](p7-automatic-boundary-acceptance.md)；测试定价不代表 Kimi 实际账单。

## 通过的功能证据

| 范围 | 实际验证 | 主要通过证据 |
|---|---|---|
| CPU Embedding | 真实 512 维有限、非恒定向量；中文相关性排序；错误 Key 拒绝 | `p7passed1008`，`test_cpu_embedding.py` |
| Mem0 CPU 基础 | `infer=False` 写入、搜索、用户过滤、删除；未授权配置返回 401 | `p7passed1008`，`test_memory_cpu.py` |
| 文本应用 | 真实 SSE 增量，最终消息与增量一致，真实 Token 信息；中途停止并收敛为取消 | `live-kimi-stream.json`、`live-kimi-cancel.json` |
| 会话与画布 | 增量可见、停止、刷新历史、Trace；Studio 增量与最终输出一致 | `p7passed1008` 内部 6 项模型浏览器用例 |
| Judge/比较 | V1/V2 各 3 条真实 Kimi 用例，基础规则、Judge 理由和 Trace；2 个共同用例及两侧各自缺失用例 | `live-kimi-evaluation.json`、浏览器比较报告 |
| Insights | 成功、Token、耗时、工作流筛选；错误链接保留工作流和错误码，六图表无预算错误 | `live-kimi-insights.json`、浏览器 Trace |
| LightRAG | 独特内容上传、真实索引/Chunk 检索；重复、非法及 8 MiB+1 文件拒绝；索引中禁止删除，终态删除 | `live-lightrag-index.json` |
| 文档与应用 | 删除记录后绑定仍有效；再上传新文档，已发布应用能检索新内容，新的调试仍返回正确的会话校验错误 | `p7mutable1008/live-document-resource-lifecycle.json` |
| Agent 记忆/隔离 | 真实知识工具、Mem0 推断写入、新会话召回；另一登录用户查询为空；撤销角色权限后新登录 Token 不能调用 | `p7isolation1008/live-agent-knowledge-memory.json` |
| 调试主体校验 | 缺可信 Application Session 返回 `AGENT_SESSION_REQUIRED`，实际 Mem0 调用为 0 | `live-debug-subject-refusal.json` |
| Python 沙箱 | 门户授权/配置/保存/运行，真实 Python 输出、stdout、Trace，Sandbox 回收 | `p7closure1008` 内部 3 项桌面浏览器用例 |
| 桌面基础/权限 | 八个入口，中英文、浅深主题、404；跨部门读取隐藏或拒绝，无权账号管理拒绝 | `local-desktop-acceptance.spec.ts` |

## 本轮修复

1. 门户 Nginx 默认 1 MiB 限制提前拦截上传。`/api/` 与既有 Ingress 对齐为 50 MiB，知识文档仍执行后端 8 MiB 校验；真实超限请求返回 `422 KNOWLEDGE_DOCUMENT_TOO_LARGE`。
2. Agent Core 同步 Port 的 `block_in_place` 与租约心跳原来在同一异步任务，长工具链丢失 Attempt Lease。适配器改为独立 `spawn_blocking`，续租、取消、Drain 可独立调度，保留 Core 的 Deadline、预算及权威 Attempt 校验。终态会清空心跳/锁字段，测试验证长调用成功及租约释放。[Tokio 调度说明](https://docs.rs/tokio/latest/tokio/task/fn.block_in_place.html)。
3. `knowledge_search` 原来使用 LightRAG `/query` 生成答案而没有真实 Chunk。改用共享 `retrieve` 协议、`/query/data` 和检索归一器，增加实际响应包络到 Citation 的回归。
4. 直接 API 探针确认 Kimi `k3` 要求 `temperature=1`、`top_p=0.95`。Mem0 测试配置按此调整，未修改 Agentx 通用模型协议。
5. Insights 原来同时请求六个聚合，超过租户四并发限制。页面改为按序请求同一快照，使用 AbortSignal 取消过时筛选；错误图表展示失败状态。后端四并发预算保持不变。
6. 文档上传/删除原来递增资源配置版本，导致 Runtime Hash 冲突。文档变动现更新文档和索引状态、保留 `rag_resources.version`；符合当前“绑定外部 workspace、检索当前数据、不做知识库数据版本化发布”的边界。删除平台记录仍不清理外部索引。
7. 新用户和角色变更原来只同步 User Admission，没有已有应用 Grant。IAM 同一事务现写入应用 Grant Outbox，复用发布时的可见性 SQL 与应用递增 Admission Epoch；新用户能调用已授权应用，撤销后新登录 Token 也被拒绝。
8. Sandbox Manager 在执行前检查标准 `/ping`，最多等待 30 秒，仅重试只读健康检查；代码命令只发送一次。真实 Python 冷启动复测通过，Sandbox 正常终止。
9. 增加离线 CPU Embedding Addon、临时服务明确网络策略；OpenSandbox 自动启动固定可达 endpoint、SQLite 与目录白名单并避免本地代理干扰。镜像导入工具为实际 OCI manifest digest 注册引用，按 digest 部署已验证。

## 原始运行记录

基线 `78bb47fc9858bd8035f37a0a468e5e388090151d`。本轮未提交或 push。各 Run 保存候选身份、镜像清单和失败证据，专项结果不能拼成九域同 Run 认证。

| Run | 原始结果/复测原因 |
|---|---|
| `p7accept1008` | 10 passed、1 failed 后停止；基础服务、Kimi、Judge、Python 通过，暴露 Agent 检索包络问题 |
| `p7complete1008` | 4 passed、1 failed；六个真实模型浏览器用例通过，Mem0 暴露 top_p 不兼容 |
| `p7memoryok1008` | 3 passed、1 failed；写入/召回通过，新用户权限投影缺失 |
| `p7passed1008` | 12 passed、2 failed；沙箱启动执行失败，另一个失败来自新增撤销测试误用 HTTP 方法 |
| `p7closure1008` | 4 passed、1 failed；Python/桌面 3 项通过，新增撤销测试 URL 拼错，已修正 |
| `p7mutable1008` | 3 passed、1 failed；文档生命周期通过，记忆及撤销业务断言通过；末尾统计错误读取已清空心跳导致失败 |
| `p7isolation1008` | 1 passed、0 failed、0 error、0 skipped，505.391 秒；用成功状态及租约释放代替已清空的心跳时间 |

Runtime 库 159 passed、2 ignored，Control 回归 62 passed；受影响 Rust Target Clippy（`-D warnings`）、格式、前端构建、浏览器 TypeScript 检查和新增 Python Ruff 检查通过。前端 lint 有仓库既有警告、无错误。全仓 Python 格式的既有 `test_channel_configuration.py` 差异未修改。

仓库 Acceptance 曾为 138 passed、1 failed：既有受 Git 跟踪的 `skills-lock.json` 违反根目录布局。本轮保留该文件，未记为通过。

过程证据：`.local/artifacts/plan7-live-text-20261008/`。集群/业务报告：`.local/artifacts/e2e/<run_id>/`。截图、录像及 Trace：`.local/artifacts/playwright/helm-agentxctl/<run_id>/`。

## 本地升级和入口

正式 Control、Runtime、Dependencies 已按摘要升级，既有 Secret 引用相同，未执行密钥轮换；配置为 `.local/artifacts/plan7-live-text-20261008/local-provider-mutable-values.yaml`。

| 服务 | 镜像摘要 |
|---|---|
| web-console | `sha256:c5339b3bfd0c44a7357939b73e33eaa0a0577759c1c691fd89f4847d34f8ec19` |
| platform-control | `sha256:e9ab44092e2707fa01efb7149d2d0b0579f66c58787c7b424700b6f7b3c01bad` |
| workflow-worker | `sha256:3c5001a8040ee28ba861fc73cfa5b440a2a6b490aa6eb48eb57e033a08de58b6` |
| sandbox-manager | `sha256:3176c4e91999ee0563be3d2e9bdb7d73a7d2315853be6c8a17b726cf04e274c7` |

Docker Desktop 原生 Cloud Provider 无法解析 kind/bridge 双网络节点，原 Ingress 长期 pending；bridge 承载独立 Sandbox 私网入口。正式 Ingress 改用既有 MetalLB Class，VIP `172.18.255.242`，由固定 Envoy TCP 前端提供宿主机入口。平台配置见 [local-ingress Addon](../../../deploy/kustomize/addons/local-ingress/README.md)，部署边界说明已同步。

内部 VIP 与实际 Envoy 转发均以原 CA 校验证书：门户/Bootstrap 200、`required=false`，Runtime 无授权请求 401。临时转发检查容器已删除。这验证准备好的配置可用，尚不代表宿主机 443 已恢复。

**入口待本机确认**：缺少 `/var/run/com.docker.vmnetd.sock`，绑定 80/443 失败，账户没有免密 sudo。已准备的 `agentx-local-ingress` 容器为 `created`。需在 Docker Desktop → Settings → Advanced 启用“Allow privileged port mapping”，由用户在本机完成管理员确认；随后启动容器并检查 `https://agentx.localhost`。[Docker 权限说明](https://docs.docker.com/desktop/setup/install/mac-permission-requirements/#binding-privileged-ports)。

## 第二批：授权、记忆边界与组合流程（2026-10-08）

本批沿用 Kimi 和 CPU Embedding，通过独立 Namespace 补齐下列场景。`p7nextfinal1008` 为 7 passed、1 failed，619.175 秒；其中失败发生在旧 Token 用例的准备阶段，只等待 Application Grant、未等待 User Admission 的 Token Version，已修正等待条件。`p7nextclosure1008` 为 2 passed、0 failed／error／skipped，498.965 秒，旧 Token 和加强后的组合链路均通过。第二批 8 个 pytest 场景均有通过证据，内部新增 1 个 Playwright 场景；原始失败保留，临时 Namespace 已全部清理。

| 场景 | 当前证据 |
|---|---|
| 桌面会签、跨部门脱敏、通过／拒绝、真实日期与审批历史 | `p7nextfinal1008`，内部 Playwright 1 passed |
| 全部部门通过才原子授权，审批后真实 Kimi 执行，撤销凭证 Grant 后新执行拒绝 | `live-grant-lifecycle.json` |
| 取消后禁止再审批；依赖停用、凭证轮换、申请人失去编辑权后标记 stale 且不创建 Grant | `live-grant-stale*.json` |
| 撤销后旧 Token 不能新建会话、不能向既有会话发消息、不能调用 Control | `p7nextclosure1008/live-old-token-revocation.json` |
| 同一用户跨应用及跨 Namespace 隔离；改绑再切回后原记忆仍能召回 | `live-memory-scope-matrix.json`，9 次真实 Mem0 调用、3 个不同 scope hash |
| 知识 Agent → 记忆 Agent → Python → 输出；真实工具、Python 验证上游值、stdout 与回收 | `p7nextclosure1008/live-agent-python-pipeline.json`，Mem0 实际写入、RAG／Memory Trace、Python 输出与回收均通过 |

本轮修复了取消后仍能写入部门 Review、停用依赖仍可被审批通过两项缺陷。审批前锁定申请主记录，校验完整依赖包指纹、资源状态、Workflow 与申请人权限；Grant 的到期时间不是当前契约字段，`stale` 表示申请依据失效。审批动作在同一事务写既有审计表，修复原先空历史及时间数组，返回 RFC 3339 时间。模块化验证与审计分别位于 `resource_request_validation.rs` 与 `resource_request_audit.rs`。

首次记忆测试捕获 Kimi 仅口头确认保存、没有发出 `memory_write`；加强测试工作流指令，并检查实际工具调用和 Mem0 的 ADD／UPDATE 结果。Agent 工具选择仍由模型决定，本批功能通过不证明其每次都遵循指令。原始失败保留。

`p7next1008` 在命名空间改绑发布时遇到节点磁盘仅余 1.7 GiB、对象存储 507，后续上传失败；并发重试 `p7nextstale1008` 也未通过安装 Doctor。清理本批失败 Namespace 与 8.659 GB 可回收 BuildKit 缓存后，节点可用空间恢复约 16 GiB，改为串行测试。保留用户镜像与卷，未降低存储门禁。

Control 回归 62 passed、Fixture 7 passed，Clippy（`-D warnings`）、Rust 格式、Python Ruff、前端构建、浏览器 TypeScript 检查通过；前端 lint 保留既有警告。新增构建摘要：Control `sha256:07d8dbe0cd5cc5456a18be6501255400258da43915011c6d83e4697b35a9f5d5`，Web `sha256:b83ca367447392d073aca79ab1ea79e74699a377db2cc890a683ec1994e385bd`。本地正式 Control 已升级至 revision 20，Control／Web 使用以上新摘要，正式业务 9 个 Deployment 全部 Ready；Secret 配置引用与升级前相同，未执行密钥轮换。最终记录为本批目录 `final-status.json`。

本批过程证据位于 `.local/artifacts/plan7-live-next-20261008/`；各业务 JSON 位于 `.local/artifacts/e2e/<run_id>/product/`。专项复测不替代同 Run 九域生产认证。第二批结束时保留的 MCP／Skill 完整依赖与权限提升矩阵已在 2026-10-09 第三批补齐。最终功能覆盖收据为 `functional-coverage.json`；旧 Token 场景额外等待 User Admission 与 Token Version 完整同步，符合执行面异步投影的拒绝策略。

## 剩余范围和资源

本节已按 2026-10-09 的 [第三批自动化验收](p7-automatic-boundary-acceptance.md) 更新。

- RAGFlow：还需可运行服务及至少一个已索引 Dataset，复用本轮 Kimi/CPU Embedding即可。本机缓存为大型 AMD64 镜像，未作为本轮 ARM64 服务运行。
- Model／Credential 授权包、旧 Token 拒绝，以及 MCP／Skill 递归依赖、选定版本、会签、拒绝/取消/stale、撤销与权限提升边界均已有专项通过证据。
- 正式沙箱强隔离需要 Kubernetes OpenSandbox Runtime 及批准的 RuntimeClass；本机 Docker/runc 仅功能测试。
- 非零定价、Redis 水位/停服/恢复及本地 Sandbox Handle 边界已补测通过。生产强隔离相关安全矩阵、固定硬件两小时、九域同 Run、历史升级/恢复、公网 OIDC 供应链认证仍未完成。
- 图片/音频和其他模型按用户要求暂缓，真实 IM 回复由用户测试。
- 宿主机 80/443 特权映射需上述本机系统确认，隔离 Namespace 的自动化已完成。

临时服务和测试 Namespace 已按编排清理：集群剩余 `agentx-e2e-*` Namespace 为 0，本地 OpenSandbox 实例为 0；正式业务 9 个 Deployment 全部 Ready。持久本地 OpenSandbox Server 留作后续功能测试。最终状态与清理记录为 `.local/artifacts/plan7-live-text-20261008/final-acceptance-summary.json`，镜像摘要为同目录 `local-verified-workloads.json`，凭证脱敏检查为 `final-keyscan.json`。
