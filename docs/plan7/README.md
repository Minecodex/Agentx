# plan7：产品最后一公里与生产收口实施计划

plan7 覆盖五个经过 n8n 与 Dify 源码对比研究后选定的最高投入产出比事项。研究结论（2026-09，基于三方源码通读）：

1. Agentx 的执行内核、可靠性与治理（Checkpoint-Fork、审批会签、lease/fencing、配额审计）已达到或超过 n8n/Dify 同类实现；
2. 差距集中在产品最后一公里（IM 出站回复、LLM 流式、终端用户体验）与生产认证收口；
3. 生态差距（连接器数量、插件市场）不通过追赶解决，由既有 MCP + 画布插件路线承接，不在本计划内。
4. 2026-09-26 完成五线全量源码勘察：各线"现状事实"已逐项核实并修正（修正内容以"勘察修正/勘察新增/勘察利好"标注直接并入各线正文），关键实现设计与先决决策见各线文档与本文 §2。

## 1. 五条工作线

| 线 | 名称 | 详细文档 | 核心目标 | 主要改动面 |
|---|---|---|---|---|
| P7-A | IM 渠道出站回复闭环 | [01-channel-outbound-reply.md](01-channel-outbound-reply.md) | 钉钉/飞书/企微机器人"能进能出"：Delivery Outbox、回复节点、投递重试与状态查询 | runtime（stream/webhook/outbox）、control（渠道配置）、web（渠道详情） |
| P7-B | LLM 流式输出与多模态透传 | [02-model-streaming-multimodal.md](02-model-streaming-multimodal.md) | 模型输出 token 级流式到客户端 SSE；image/audio 内容原生进模型请求 | runtime worker/gateway、agent-core、模型资源契约、web playground |
| P7-C | 评测与分析面补全 | [03-evaluation-insights.md](03-evaluation-insights.md) | llm_judge 进 UI、评测版本对比报告、跨执行 Insights 聚合页 | control（BFF/对比 API）、observability（维度修复）、web |
| P7-D | P0 收口与生产认证 | [04-production-closeout.md](04-production-closeout.md) | 关闭 `docs/todolist.md` 唯一条目；关闭 V2-06B/07B/08B 与 M7/INT-009/011/012/014 全部剩余门禁 | tests/e2e、runtime（背压）、tools/agentxctl、release 脚本 |
| P7-E | 知识库管理面 | [05-knowledge-management.md](05-knowledge-management.md) | 知识库从"连接"升级为文档管理 + 索引状态 + hit-testing 的最小闭环 | control（rag 文档 API）、runtime（协议抽取）、web（knowledge 详情） |

## 2. 实施顺序

```text
P7-D1/D2（provider 测试收口 + 容量阈值冻结）   ← 最先，收口 todolist 与门禁前置
   │
   ├─ P7-A（出站回复闭环）                      ← 独立推进，性价比最高
   │
   ├─ P7-B（流式 + 多模态）                     ← 独立推进，A 完成后聊天体验完整
   │
   ├─ P7-E（知识库管理面）                      ← 复用 D1 的 provider fixture
   │
   └─ P7-C（评测与 Insights）                   ← 依赖观测面维度修复，可与上述并行
P7-D3~D8（背压、容量、升级、恢复、供应链、发布审查）← 贯穿全程，最后统一收口
```

依赖关系只有两条硬约束：

1. P7-C 的 Insights 聚合依赖"Trace 事件补 `workflow_id`/`application_id` 维度"（详见 [03](03-evaluation-insights.md)），该修复同时惠及评测失败节点分布；
2. P7-E 的 E2E 复用 P7-D1 固化的 LightRAG/RAGFlow fixture。

其余各线互不依赖，可并行。

2026-09-26 全量源码勘察后确定的五个先决设计（已并入各线文档，实施时不再重新讨论）：

| # | 分叉 | 定案 |
|---|---|---|
| 1 | P7-A 平台 API 域名管控 | gateway 无主机白名单且公网 443 已放行；域名后缀校验放 delivery Send Client 内实现（见 01 §3.4） |
| 2 | P7-B content 枚举兼容 | `#[serde(untagged)]` Text 优先 + `CORE_CONTRACT_VERSION` 1.1→1.2（见 02 §3.5） |
| 3 | P7-C 错误跳转 | 新增 errorCodes 全链路筛选（前端 URL 参数 → BFF → `ExecutionSearchRequestV1`，见 03 §2.3/§3.3） |
| 4 | P7-D 供应链镜像数 | supply-chain schema 对齐 11 镜像（8 常驻 + 3 Job，见 04 §2.3） |
| 5 | P7-E 出网路径 | Control 直连（集群内 dependencies），补 NetworkPolicy + Vault 读 + endpoint 校验三件（见 05 §2） |

## 3. 全局约束

沿用仓库现行规则，本计划所有任务必须遵守：

1. 前后端单文件不超过 2000 行；新模块按关注点拆分。
2. 不保留向后兼容：过时表、DTO、API 直接删除，不写迁移层和 fallback（项目处于开发期）。
3. 跨平台自动化只写 Python；Kubernetes 系统级 E2E 以 `pytest tests/e2e` 为唯一编排入口，浏览器操作走 `tests/browser` Playwright；测试用临时 Namespace，结束后删除并恢复被缩容的开发服务。
4. 契约先行：改 `contracts/openapi` 后必须重新生成 `src/web/src/shared/api/generated.ts`；协议/Schema 冻结以契约测试为准。
5. 本地证据统一写 `.local/artifacts/e2e/<run_id>/`，证据不得包含 Secret、Token、完整请求体。
6. 每条线完成时：契约测试、静态边界检查（`cargo xtask check`）、Rust 单测、前端 vitest、Kubernetes E2E 全绿后才允许勾选任务与更新本文档状态。

## 4. 状态与证据

| 线 | 状态 | 证据 |
|---|---|---|
| P7-A | **代码+门禁完成；E2E 主链全绿** | A7 `1 passed 233s`(skipped=0)：钉钉入站→执行→L1 回复→im-mock delivered(`provider_message_id`)→401 死信 `DELIVERY_PROVIDER_REJECTED`；过程修复 9 个真实缺陷。A7 场景 2/3/4/6-9 与死信重放移交 |
| P7-B | **代码+门禁完成；B6 主链全绿** | B6 `1 passed 373s`：慢流/断流/无 usage/重放合并帧四行为，delta 先于终态按序。**已知缺口**：`first_token_ms` Trace 字段与 `x-agentx-modality` 标记未实现（移交）；场景 5-8 移交 |
| P7-C | **代码+门禁完成** | C1-C4 实现全落地（insights_api/对比 API/llm_judge UI/维度修复），vitest 与 Playwright 域内绿；无独立集群 E2E（按计划门禁为契约+testcontainers+vitest+Playwright） |
| P7-D | **D1/D2/D3/D5/D6 全绿；D4/D7/D8 部分完成** | D1 `2 passed`、D2 阈值冻结、D3 熔断/公平限流+准入 429、D5/D6 演练 `8 passed`；D4 分级容量 Run/2h 稳定性、D7 真实签名链+攻击矩阵、D8 全链汇总 Run 待窗口执行（移交清单见 evidence） |
| P7-E | **代码+门禁完成；E6 全绿** | E6 `1 passed 389s`：上传→索引→hit-testing 命中+sapphire-orbit 内容；修复 4 个真实缺陷（LightRAG v0.20 协议实测）；RAGFlow 组合联调移交 |

证据文件在各线完成后写入 `docs/plan7/evidence/`，命名 `p7-<线号>-<主题>.md`。

## 5. 与既有计划的关系

- plan7 不回写 plan4/planv2/plan5/plan6 的历史状态；P7-A 落地后 plan4 §20 的"方案讨论"章节由本计划 [01](01-channel-outbound-reply.md) 取代为正式契约。
- P7-D 是 planv2 V2-06B/07B/08B 与 `docs/plan/12-integration-hardening-release.md` M7 剩余项的唯一收口入口；两矩阵的 done 判定仍以 `docs/planv2/99-traceability.md` 和 `docs/plan/99-feature-traceability.md` 为准，plan7 只负责交付证据。
- 评测报告能力对齐 `docs/05-platform-business.md` §12 已宣称范围，属于还账而非新增需求。

## 6. 全局完成定义

plan7 完成必须同时满足：

1. 五条线详细文档中所有任务勾选完成，各自完成定义逐条满足；
2. 钉钉/飞书/企微入站消息可收到工作流结果的平台回复，投递失败可查、可重试、幂等；
3. Playground 对话逐 token 流式渲染；带图片的消息在模型请求中以原生多模态 content 传递；
4. 评测 Profile 可在 UI 配置 llm_judge；评测报告可对比两个 Run；Insights 页可查错误分布、成本趋势、成功率和节点耗时；
5. `docs/todolist.md` 清空；`docs/planv2/99-traceability.md` 与 `docs/plan/99-feature-traceability.md` 无 planned/in_progress/blocked 行（或明确移交下一计划）；
6. 全部 E2E 以 `failures=0、skipped=0` 通过，证据可复现。
