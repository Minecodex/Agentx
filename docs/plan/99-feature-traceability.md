# 首期功能追踪矩阵

本文件将产品能力唯一映射到实施阶段、服务、页面、存储、权限和测试。状态只允许 `planned`、`in_progress`、`blocked` 和 `done`；验收证据在任务完成时补充测试路径、命令或发布记录。

## 1. 首期能力矩阵

| 能力 | 产品/架构来源 | 阶段与任务 | 后端责任 | 前端入口 | 主要存储 | 权限 | 关键测试 | 状态/证据 |
|---|---|---|---|---|---|---|---|---|
| 单公司初始化、登录和租户上下文 | 01 §4、07 §5 | IAM-001–003、IAM-007、IAM-009 | platform-api | `/setup`、`/login`、`/change-password` | MySQL、Refresh Cookie | bootstrap、auth | 初始化、Token 轮换、tenant_id 伪造 | done / [M1 验收证据](m1-acceptance-evidence.md) |
| 部门、用户、角色和数据范围 | 01 §4、05 §1 | IAM-004–005、IAM-010 | platform-api | `/organization`、`/roles` | MySQL | user/role/department manage | 部门闭包、权限矩阵、跨租户 Repository | done / [M1 验收证据](m1-acceptance-evidence.md) |
| Workflow 草稿、版本、部署和回滚 | 01 §2–4、03 §2/6 | WCP-001–010；M2R-070～076 | platform-api | `/workflows` 及详情/画布 | MySQL、MinIO | workflow create/edit/publish | Revision 冲突、MCP 连线、版本不可变、发布 | done / [M2.1 验收证据](m2.1-acceptance-evidence.md) |
| Credential、Model、MCP、Skill 和资源授权 | 01 §5、05 §2–5 | RES-001–011；M2R-020～076；M2F-001～005 | platform-api、echo-mcp | Credential、Model、MCP、Skill Workspace、统一资源授权 | MySQL、MinIO | resource manage/grant、mcp discover/debug | Secret 脱敏、连接状态、Schema 字段树、富文本、MCP 三段 Grant | done / [M2.1 验收证据](m2.1-acceptance-evidence.md) |
| 画布设计期资源授权申请与部门会签 | 05 §2、06 §5/7、10 §7/10 | resource-grant-requests | platform-api、Workflow Studio、Approvals | ResourcePicker 六态、完整依赖授权包、跨部门会签、通知和审计 | MySQL、TanStack Query、Radix Popover | workflow edit；resource grant；approval act | 直接授权/申请不自动选中，审批复核版本与编辑权，撤权保留引用并阻止保存/运行 | done / [资源授权申请验收证据](resource-grant-requests-acceptance-evidence.md) |
| LightRAG、Mem0 控制面和可选 Addon | 01 §4、05 §6–7 | RES-005–011；M2R-080～086 | platform-api | `/knowledge`、`/memory` | MySQL、Kubernetes PVC | rag/memory manage/grant | 连接、读写范围、Addon 渲染与健康 | done / [M2.1 验收证据](m2.1-acceptance-evidence.md) |
| Application、API Key、Session、Gateway 和 SSE 外围契约 | 01 §2/4、05 §8–10 | APP-001–011 | platform-api、trigger-gateway | `/applications`、`/playground` | MySQL、Redis、MinIO | application manage/invoke | Key、幂等、Session 固定版本、SSE 重连、Runtime 不可用零伪记录 | done / [M3 验收证据](m3-acceptance-evidence.md) |
| Application Invocation 到真实 Execution | 01 §2/4、05 §8–10 | INT-001 | trigger-gateway、coordinator、worker | `/playground`、Execution | MySQL、Redis、ClickHouse | application invoke/workflow run | Message、Invocation、Execution 和 Trace 双向定位 | done / [M7 验收证据](m7-acceptance-evidence.md) |
| Dataset、Evaluation Profile Version、内嵌评分规则和空报告 | 01 §2/4、05 §11–12 | EVA-001–010、M3F-001～005 | platform-api | `/datasets`、`/evaluations` | MySQL、MinIO | dataset/evaluation_profile/evaluation manage | Dataset/Profile 版本不可变、导入原子、四类规则、Runtime 不可用零 Case Result | done / [M3.1 验收证据](m3.1-acceptance-evidence.md) |
| Evaluation 批量真实 Execution 和指标 | 01 §2/4、05 §11–12 | INT-003 | coordinator、workflow-worker | `/evaluations` | MySQL、ClickHouse | evaluation manage/workflow run | Case Execution、成本、耗时和 Trace 指标 | done / [M7 验收证据](m7-acceptance-evidence.md) |
| 审批任务和站内通知外围能力 | 01 §1/4、04 §11、05 §13 | OBS-001–003 | platform-api | `/approvals`、消息中心 | MySQL、Redis | approval act/manage、notification view | 并发终态、候选资格、幂等投影和业务跳转 | done / [M3 验收证据](m3-acceptance-evidence.md) |
| Approval Node 恢复和运行通知闭环 | 01 §1/4、04 §11、05 §13 | REC-005–006、INT-002/007 | coordinator、worker | `/approvals`、Execution | MySQL、Redis、ClickHouse | approval act/workflow run | 决策后恢复、输出端口和重复事件 | done / [M7 验收证据](m7-acceptance-evidence.md) |
| Item、表达式、Node Protocol 和基础节点 | 01 §4、03 §3–7/13 | RUN-001–005、RUN-012 | runtime、worker | Studio 参数和输出面板 | MySQL、MinIO | workflow edit/run | 多来源 Lineage、AST 安全、Manifest/Action 版本、协议一致性 | done / [M4 验收证据](m4-acceptance-evidence.md) |
| IF、Switch、Readiness、Merge、普通图环、Loop、Wait、Sub-workflow | 01 §4、03 §7–10 | RUN-006–008、REC-004、WCP-004 | runtime、coordinator、worker | Workflow Studio | MySQL、Redis | workflow edit/run | M4 `n8n_v1` 基线；M6 引入显式 Connection Order、ClosedWithoutData、required inputs、回边激活预算和固定子版本；现行 Definition 4.0 保留这些语义 | done / [M4 验收证据](m4-acceptance-evidence.md)、[Workflow 5.0 验收证据](workflow4-acceptance-evidence.md) |
| API、Webhook、Schedule 和 SSE 触发契约 | 01 §4、05 §10 | APP-006–008 | trigger-gateway | Application、Playground | MySQL、Redis | application invoke | JWT/API Key、触发幂等、Webhook 防重放和 SSE 回放 | done / [M3 验收证据](m3-acceptance-evidence.md) |
| Schedule、Webhook、Poll 和手动触发到真实调度 | 01 §4、03 §13、05 §10 | RUN-002、RUN-010–014、INT-004 | trigger-gateway、coordinator | Application、Studio | MySQL、Redis | application invoke/workflow run | activate/deactivate、扫描、Webhook/Poll、派发、取消和状态一致性 | done / [M7 验收证据](m7-acceptance-evidence.md) |
| Execution、Node Activation/Attempt/Delivery 和可靠调度 | 01 §2/3、03 §8–11 | RUN-006、RUN-009–014 | coordinator、worker | `/executions` | MySQL、Redis | workflow view_execution | 分支顺序、重复 Delivery、Lease、崩溃、超时 | done / [M4 验收证据](m4-acceptance-evidence.md) |
| Execution Summary、Trace Pipeline、查询和 Artifact | 01 §4、04 §1–4 | OBS-004–006 | trace-writer、platform-api | Execution Trace | MySQL、Redis、ClickHouse、MinIO | execution/trace view | 脱敏、Outbox 交付、Span 树、Artifact 授权 | done / [M3 验收证据](m3-acceptance-evidence.md) |
| 模型成本、MCP 错误和 Agent 循环运行 Trace | 01 §4、04 §8–9 | AGT-006–007/012 | worker、trace-writer | Execution Trace | MySQL、ClickHouse、MinIO | workflow run/trace view | Token、成本、重复错误、A-B-A-B、ClickHouse 中断补投和查询脱敏 | done / [M5 验收证据](m5-acceptance-evidence.md) |
| Checkpoint、Fork 和部分执行 | 01 §1/4、04 §5–7 | REC-001–003、REC-007–010 | coordinator、worker | Execution、Studio | MySQL、MinIO | workflow fork_execution | 原记录不变、依赖恢复、副作用确认 | done / [M4 验收证据](m4-acceptance-evidence.md) |
| Model、Agent、MCP Tool、Skill、RAG 和 Memory 运行 | 01 §1/4、04 §8–9、05 §3–7 | AGT-001–007、AGT-012–013、INT-005 | worker、infrastructure | Execution Workbench、Trace | MySQL、ClickHouse、MinIO | resource grant/workflow run | 运行时授权、撤权、预算、循环、Attempt 恢复、递归依赖和固定版本 Addon | done / [M5 验收证据](m5-acceptance-evidence.md)、[M7 验收证据](m7-acceptance-evidence.md) |
| OpenSandbox 与 Code Runtime 功能基线 | 01 §1/4、04 §10、07 §11 | AGT-008–011/013 | sandbox-manager、worker | Execution Workbench、Trace | MySQL、Redis、MinIO、OpenSandbox | workflow run/resource grant | Rust 直接 Adapter、Spec/差分契约、SSE/Endpoint、Docker+runc 功能 E2E、内存/TTL/配额、默认拒绝网络、Secret 和强制回收 | done / [M5 验收证据](m5-acceptance-evidence.md) |
| Sandbox 生产强隔离与供应链 | 04 §10、07 §6–11 | INT-006/010/011/014 | sandbox-manager、worker、deployment | Runtime Status | MySQL、OpenSandbox、Kubernetes | tenant admin/workflow run | RuntimeClass、CPU/内存/PID/磁盘/TTL、双栈 egress、Vault、签名镜像和两租户攻击矩阵 | in_progress / [M7 验收证据](m7-acceptance-evidence.md)；runc `standard` 基线已通过，本地供应链验收器已实现但真实 Run 尚未执行 |
| n8n-like 画布、Agent/Code 编排、Draft 调试和发布 | 01 §1/2、10 §7–10 | STU-001–016 | platform-api、runtime services | `/workflows/:id/editor` | MySQL、Redis、MinIO | workflow edit/run/publish/resource grant | Definition 4.0/Editor/Overlay 分离、Catalog Manifest 表单、Draft Revision Snapshot、真实拖拽/连线、Pin、运行高亮、Trace 定位和发布 | done / [M6 验收证据](m6-acceptance-evidence.md)、[Workflow 5.0 验收证据](workflow4-acceptance-evidence.md) |
| Workflow 5.0 Start/End、Expression 与 Context | 03 §3–10、12 | Workflow 5.0 §1–6 | domain、runtime、coordinator、worker | Workflow Studio | MySQL、Redis、MinIO | workflow edit/run | Inputs/Outputs/Contexts、类型/基数、Patch/CAS、Reference Picker | done / [Workflow 5.0 验收证据](workflow4-acceptance-evidence.md) |
| Workflow 发布为 Composite Node 与 Package 移植 | 05 §8–10、12 | Workflow 5.0 §7–9 | platform-api、trigger-gateway、runtime | Workflow Studio、Application | MySQL、MinIO | workflow publish/invoke | 固定子版本、Overlay、递归检测、签名导入、资源重绑定、Multipart | done / [Workflow 5.0 验收证据](workflow4-acceptance-evidence.md) |
| 配额、保留、扩容和发布 | 07 §2/6–9 | INT-006、INT-008–014 | 全部服务 | Runtime Status | MySQL、Redis、ClickHouse、MinIO、K8s | tenant admin | 故障、容量、清理、滚动升级 | in_progress / [M7 验收证据](m7-acceptance-evidence.md)；INT-009/011 本地验收器已实现（plan7 又补齐滚动升级持续探针/未知协议不领取与 PITR+Redis 重建演练用例，见 docs/plan7/evidence/p7-implementation-status.md），INT-012/014 不在本轮关闭 |
| 业务域国际化与安全删除 | 05 §14、06 §12、10 §14–15 | 横切控制面能力 | platform-api、web | 全部可管理实体列表 | MySQL | 独立 `*:delete` | 双语键隔离、引用预检、事务复检、竞态、聚合清理、临时 K8s UI E2E | done / [安全删除验收证据](safe-deletion-acceptance-evidence.md) |

## 2. MVP 十二步映射

| MVP 步骤 | 唯一验收阶段 | 前置任务 | 证据要求 | 状态 |
|---|---|---|---|---|
| 1. 初始化企业和 Admin | 02 | IAM-001–003、IAM-009 | [M1 验收证据](m1-acceptance-evidence.md) | done |
| 2. 创建部门、用户和角色 | 02 | IAM-004–005、IAM-010 | [M1 验收证据](m1-acceptance-evidence.md) | done |
| 3. 接入 Credential、模型、MCP、Skill Workspace、LightRAG 和 Mem0 | 04 | RES-001–011、M2R-020～086 | [M2.1 验收证据](m2.1-acceptance-evidence.md) | done |
| 4. 将直接和递归资源依赖授权给 Workflow | 04 | WCP-006、RES-007/011、M2R-070～076、M2F-001/003/005 | [M2.1 验收证据](m2.1-acceptance-evidence.md) | done |
| 5. 拖拽 Agent、MCP Tool、Code 和审批 Workflow | 11 | STU-002、STU-005–010、STU-016、AGT-013 | Studio E2E 和 Definition/Editor Snapshot | done / [M6 验收证据](m6-acceptance-evidence.md) |
| 6. 手动运行并查看节点输入输出 | 11 | STU-003、STU-011–013、RUN-014 | Draft Revision 真实 Execution/Node Trace | done / [M6 验收证据](m6-acceptance-evidence.md) |
| 7. 查看模型、Tool、成本和循环 Trace | 10 | AGT-006–007/012、OBS-006 | [M5 验收证据](m5-acceptance-evidence.md) | done |
| 8. 从历史 Checkpoint 重新执行 | 09 | REC-001–003、REC-010 | [M4 验收证据](m4-acceptance-evidence.md) | done |
| 9. 用 Dataset 批量评测版本 | 12 | EVA-001–010、INT-003 | Case Execution 和报告指标 | done / [M7 验收证据](m7-acceptance-evidence.md) |
| 10. 发布为 Application | 12 | WCP-005/007、APP-001、INT-001 | Deployment 与调用记录 | done / [M7 验收证据](m7-acceptance-evidence.md) |
| 11. Playground/API 建立 Session 并发送消息 | 12 | APP-002–011、INT-001 | API Key、SSE、Message 和 Execution | done / [M7 验收证据](m7-acceptance-evidence.md) |
| 12. 待办审批并恢复 Workflow | 12 | OBS-001–002、REC-005–006、INT-002 | Approval Action 和恢复 Trace | done / [M7 验收证据](m7-acceptance-evidence.md) |

## 3. 维护规则

- 一项能力只能有一个主要验收阶段，可以依赖多个前置任务。
- 功能任务标记 `done` 时，同步更新本矩阵状态和证据。
- 新增首期功能时先确认产品范围，再添加唯一任务编号和验收阶段。
- 删除或延期功能时同步更新产品范围、路线图和依赖任务，不能只从矩阵移除。
- M7 发布前本文件不能存在 `planned`、`in_progress` 或 `blocked`。
