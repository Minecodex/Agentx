# Agentx 产品与架构文档

Agentx 是一个采用 n8n 式画布交互、但使用 Agentx 原生 Workflow/Node/Expression/Runtime 协议的企业 Agent 平台。首期采用单公司部署和公司内多部门模型，数据实体继续保留 tenant_id 作为隔离边界。

平台的主链路是：

设计 Workflow → 手动调试 → 查看执行记录 → 测试集评测 → 发布版本 → 生成应用和 API → 生产运行 → 审批或恢复 → 分析 Trace、成本和错误

本文档集只关注 Workflow 业务上下游，不把项目扩展为通用 OA、通用低代码平台或通用企业基础设施平台。

## 文档目录

| 文档 | 内容 |
|---|---|
| [plan/repository-layout.md](plan/repository-layout.md) | 六目录仓库结构、路径映射、产物归并与验收计划 |
| [plan/repository-layout-evidence.md](plan/repository-layout-evidence.md) | 目录迁移实际执行结果与验证边界 |
| [01-product-scope.md](01-product-scope.md) | 产品定位、边界、核心概念和业务闭环 |
| [02-system-architecture.md](02-system-architecture.md) | 系统分层、模块职责、部署单元和存储分工 |
| [03-workflow-engine.md](03-workflow-engine.md) | n8n 式数据模型、节点协议、表达式、调度和故障恢复 |
| [04-runtime-governance.md](04-runtime-governance.md) | Trace、Checkpoint、Agent 循环、OpenSandbox 和审批 |
| [05-platform-business.md](05-platform-business.md) | 多租户权限、模型与工具资源、应用会话和测试评测 |
| [06-data-model.md](06-data-model.md) | MySQL、ClickHouse、Redis、对象存储的数据设计 |
| [07-deployment.md](07-deployment.md) | Kubernetes 部署、水平扩容和运行可靠性 |
| [08-roadmap.md](08-roadmap.md) | 开发阶段、交付物、验收标准和 MVP 范围 |
| [09-codebase-architecture.md](09-codebase-architecture.md) | Monorepo、Rust 服务、公共 Crate 和依赖边界 |
| [10-frontend-architecture.md](10-frontend-architecture.md) | Tailwind UI 体系、企业工作台和 React Flow 画布 |
| [11-node-integration.md](11-node-integration.md) | Node Manifest、Action/Provider/Lifecycle HTTP 协议与接入验证 |
| [12-workflow-5.md](12-workflow-5.md) | Workflow Definition 8.0、Binding、Selector、输出契约与故障终态 |
| [13-architecture-service-data-map.md](13-architecture-service-data-map.md) | 当前架构、服务访问链路、全量表目录和跨域 ER 关系 |
| [reference/mysql-schema-catalog.md](reference/mysql-schema-catalog.md) | V2-00 逐表处置使用的历史 V1 MySQL 字段、索引与外键快照 |
| [planv2/contracts/table-disposition.json](planv2/contracts/table-disposition.json) | 历史 133 表迁往 Control/Runtime/Split/Delete 的机器可读处置输入 |
| [plan/README.md](plan/README.md) | 全量实施顺序、阶段任务、依赖、验收门禁和功能追踪 |
| [planv2/README.md](planv2/README.md) | 控制面、执行面与可观测面分离的破坏性重构计划；V2-08A 本地功能闭环与 V1 删除已完成，生产容量、安全、恢复和发布认证仍在 08B |
| [plan3/README.md](plan3/README.md) | 参考 earendil-works/pi 行为实现的 Agentx 原生内核、内置模型、OpenSandbox 四工具、会话压缩与外挂能力重构计划 |
| [plan4/README.md](plan4/README.md) | Application 渠道入站与双模式接入的历史计划；当前出站回复契约与验收见 [plan7](plan7/01-channel-outbound-reply.md) |
| [plan5/README.md](plan5/README.md) | 已完成的Workflow节点体系重构：Dify式配置与调试流程、Definition 8.0、13类节点、Agent附件、Loop容器、Approval/Exit/集成执行闭环及完整Kubernetes门禁；[implementation-plan.md](plan5/implementation-plan.md) 保存M0–M6 + F1–F8完成清单与证据 |
| [plan6/README.md](plan6/README.md) | 画布插件：动态 React UI、TypeScript/Node.js、权威动态契约、调用隔离、文件流和 Trace；SDK/RPC 2 的最新修复与验证范围见 [P6-11](plan6/evidence/p6-11-runtime-boundaries.md) |
| [plan7/README.md](plan7/README.md) | 产品最后一公里与生产收口（对比 n8n/Dify 后立项）：IM 渠道出站回复闭环、LLM 流式与多模态透传、评测与 Insights 补全、P0 生产认证收口、知识库管理面 |
| [planv2/evidence/egress-gateway.md](planv2/evidence/egress-gateway.md) | SaaS 受控公网出口、真实模型、密钥轮换、稳定性、滚动升级和生产 CNI 未关闭门禁证据 |
| [plan/m2-task-list.md](plan/m2-task-list.md) | M2 Workflow 控制面与资源中心的详细实施批次和任务清单 |
| [plan/m2.1-resource-redesign.md](plan/m2.1-resource-redesign.md) | M2.1 MCP、Skill Workspace、Kubernetes Addon 与全局 E2E 重构任务 |
| [plan/m2.1-acceptance-evidence.md](plan/m2.1-acceptance-evidence.md) | M2.1 当前验收状态和最终证据入口 |
| [plan/m3-task-list.md](plan/m3-task-list.md) | M3 应用入口、评测控制面与运行可观测外围任务 |
| [plan/m3-acceptance-evidence.md](plan/m3-acceptance-evidence.md) | M3 快速检查、Kubernetes E2E 和完成边界证据 |
| [plan/m3.1-evaluation-profile-and-prerequisites.md](plan/m3.1-evaluation-profile-and-prerequisites.md) | M3.1 评测方案合并与依赖型操作交互修正任务 |
| [plan/m3.1-acceptance-evidence.md](plan/m3.1-acceptance-evidence.md) | M3.1 契约、前端、Kubernetes E2E 和完成边界证据 |
| [plan/m4-task-list.md](plan/m4-task-list.md) | M4 可靠运行的实施批次、依赖、门禁、Kubernetes E2E 与剩余任务统计 |
| [plan/m4-acceptance-evidence.md](plan/m4-acceptance-evidence.md) | M4 Runtime、Recovery、Workbench、Kubernetes 故障注入和数据库断言证据 |
| [plan/opensandbox-feasibility.md](plan/opensandbox-feasibility.md) | OpenSandbox 当前机器兼容性、Rust SDK 缺口、直接 Adapter 决策和生产隔离边界 |
| [plan/composable-deployment.md](plan/composable-deployment.md) | 已被 Helm + Python 取代的可组合部署历史设计与验收事实 |
| [plan/m5-task-list.md](plan/m5-task-list.md) | M5 Rust 协议门禁、Agent、资源 Runtime、OpenSandbox、Trace 和 E2E 实施批次 |
| [plan/m5-acceptance-evidence.md](plan/m5-acceptance-evidence.md) | M5 当前实现、自动化证据、未关闭门禁和生产强隔离边界 |
| [plan/m6-acceptance-evidence.md](plan/m6-acceptance-evidence.md) | M6 Workflow Studio 契约、实现、静态门禁和 Kubernetes E2E 验收证据 |
| [plan/m7-acceptance-evidence.md](plan/m7-acceptance-evidence.md) | M7 业务闭环、Vault、发布自动化和剩余生产门禁证据 |
| [plan/workflow4-acceptance-evidence.md](plan/workflow4-acceptance-evidence.md) | Workflow 5.0 Definition、Expression、Context、Composite、API、Package 与 20 项验收证据 |
| [plan/resource-grant-requests-acceptance-evidence.md](plan/resource-grant-requests-acceptance-evidence.md) | 画布资源六态、直接授权、跨部门会签、脱敏、通知和运行闭环验收证据 |
| [plan/uniqueness-validation-acceptance-evidence.md](plan/uniqueness-validation-acceptance-evidence.md) | 用户输入唯一字段前检、并发兜底、字段错误、Artifact 补偿与 UI 验收证据 |
| [plan/e2e-testing-standard.md](plan/e2e-testing-standard.md) | 临时 Kubernetes Playwright 端到端测试规范 |
| [plan/safe-deletion-acceptance-evidence.md](plan/safe-deletion-acceptance-evidence.md) | 业务域国际化、统一删除契约、引用保护和临时 Kubernetes 验收证据 |
| [plan/workflow-multi-exit.md](plan/workflow-multi-exit.md) | 历史多结束节点设计记录；当前正式契约以Definition 8.0与plan5为准 |
| [plan/m2-acceptance-evidence.md](plan/m2-acceptance-evidence.md) | 已被 M2.1 取代的旧 M2 历史证据 |

## 阅读和实施顺序

1. 先阅读本页和 `01` 至 `10` 的产品、架构与工程边界。
2. 通过 [实施总计划](plan/README.md) 确认当前阶段、进入条件和全局完成定义。
3. 阅读对应阶段实施手册，按任务依赖推进并保存验收证据。
4. 当前产品能力和历史证据查 [功能追踪矩阵](plan/99-feature-traceability.md)；V2 重构实施状态查 [V2 架构与产品能力矩阵](planv2/99-traceability.md)。

现有 `01` 至 `13` 文档回答“当前系统是什么以及为什么这样设计”，`plan/` 保存历史能力和验收基线，`planv2/` 保存三域架构的实施状态与生产认证边界。[当前架构全景](13-architecture-service-data-map.md)以 V2-08A 后的代码、清单和分域 Schema 事实为准。

## 核心设计结论

1. Workflow 是平台第一核心对象，其他功能都服务于其开发、运行、发布和评测。
2. Workflow Studio 对齐 n8n 的拖拽、配置、调试和发布交互，但 Definition、IR、Manifest、表达式和节点 API 全部使用 Agentx 原生协议；首期及 M6/M7 不实现 n8n JSON、社区节点或 Credential 兼容。
3. Workflow 草稿可变，Workflow Version 不可变，Deployment 负责将版本发布到环境。
4. 采用 Item 数据模型传递节点数据，保留 Item 的上下游来源关系。
5. MySQL 保存权威业务状态；Redis 只负责缓存、租约、限流和任务派发。
6. ClickHouse 首期只保存和查询 Workflow Trace，不要求接入 Prometheus 或 Grafana。
7. Trace、Checkpoint 和审计事件是三类不同数据，不能互相替代。
8. 历史节点重新执行创建 Fork Execution，不修改原执行记录。
9. 生产 Workflow 使用独立运行身份，不继承设计者个人权限。
10. 控制面可以模块化单体起步，但 Workflow Worker、Sandbox Manager 和 Trace Writer 必须能够独立扩容。
11. 前端使用 Tailwind CSS 设计令牌和统一组件层，复杂无障碍交互基于 Radix UI。
12. Workflow 画布使用 React Flow；Workflow Definition、Editor Document 和 Debug Overlay 分离，运行编译只消费 Definition。
13. Kubernetes 核心资源由 Control、Runtime、Observability、Dependencies 四个 Helm Release管理；Rust `agentxctl`提供 Windows/Linux统一命令并嵌入固定 Chart/Schema，Kustomize只管理可选 Addon和 E2E Fixture，并通过专用 ingress-nginx暴露 Web。
14. Sandbox 采用独立安装的 OpenSandbox；本地 Docker+runc 与当前 Kubernetes 用于功能验收，gVisor/Kata 和生产强隔离作为后续强化。

## 分阶段详细设计

各规范在所属阶段冻结；已完成的阶段以验收证据和版本化 Schema 为准，未完成阶段继续按任务计划固化：

- Workflow Definition JSON Schema
- Workflow 编译后 IR
- Node Protocol/API 与节点版本兼容规则
- 表达式语法和执行上下文
- Execution 状态机
- MySQL DDL 与索引
- 外部 API 和内部 gRPC 协议
- OpenSandbox Adapter、Sandbox Profile 和安全策略
- Trace Event Schema

这些规范分别在 [阶段 01](plan/01-contracts-and-foundation.md)、[阶段 08](plan/08-workflow-runtime-core.md)、[阶段 09](plan/09-checkpoint-wait-recovery.md) 和 [阶段 10](plan/10-agent-opensandbox.md) 中完成并通过阶段门禁，不再作为无归属的开放事项保留。


## 提交与自动检查

主分支保护、功能分支、PR 和必需检查见 [PR 与 CI 合并规则](ci.md)。
