# 阶段 06：测试集和评测

此页为历史阶段记录；原全局入口、规则评分和手动发布测试数据流程已由 [应用内测试集与 AI 评测](../plan8/12-application-evaluations.md) 取代。当前实现与验收以该文档及产品主文档为准。

## 1. 目标与用户价值

提供可复现的 Dataset Version、Test Case、Evaluation Profile 和 Evaluation Report 控制面，使 Workflow Version 在引擎接入后可以直接批量评测，而不改变报告模型。

## 2. 当前状态和进入条件

- 状态：`done`。验收证据见 [M3 验收证据](m3-acceptance-evidence.md)。
- 进入条件：[阶段 03](03-workflow-control-plane.md) 提供不可变 Workflow Version，[阶段 01](01-contracts-and-foundation.md) 提供 Artifact。
- 运行引擎接入前只创建定义和显式不可用的运行请求，不生成伪报告指标。

## 3. 范围和不做内容

实现 Dataset、版本、Case、Evaluation Profile、Evaluation Run 元数据和报告查询结构。不实现通用实验平台、自动模型训练或任意 BI 报表。

## 4. 领域对象、状态和不变量

- Dataset 是可变容器；Dataset Version 创建后不可修改。
- Test Case 包含 Input、Expected Output、Context、Tags 和可选规则覆盖。
- Evaluation Profile 是用户可见的唯一评测配置，包含多条评分规则、聚合策略和通过阈值。
- Profile Version 不可变；Evaluation Run 固定 Workflow Version、Dataset Version、Profile Version 和运行参数快照。
- Run 状态为 `created`、`queued`、`running`、`completed`、`failed` 或 `cancelled`。
- Case Result 对应一个 Test Case；引擎接入后必须引用一个 Workflow Execution。
- 内置评分规则结果可复现；LLM Judge 保存模型、Prompt、参数和价格快照。

## 5. 数据和 Migration

主要表：

- datasets、dataset_versions、test_cases
- evaluation_profiles、evaluation_profile_versions、evaluation_profile_rules
- evaluation_runs、evaluation_case_results、evaluation_metrics
- evaluation_comparisons

大型输入、预期输出、批量导入文件和报告导出通过 Artifact Reference 保存。Dataset Version 保存 Case 顺序、内容 Hash 和数量快照。

## 6. REST API、Port 和事件

- `/api/v1/datasets`
- `/api/v1/datasets/{id}/versions`
- `/api/v1/dataset-versions/{id}/cases`
- `/api/v1/evaluation-profiles`
- `/api/v1/evaluations`
- `/api/v1/evaluations/{id}/results|metrics|comparison`

定义 `EvaluationRuntime` Port：创建批量 Test Execution、取消和查询进度。阶段 08 前返回 `RUNTIME_UNAVAILABLE`。

定义评分规则 Adapter Port，首期实现 Exact、Contains、Regex 和 JSON Schema；自定义代码和 LLM Judge 在阶段 10 接入运行 Adapter。评分规则不是独立的用户资源，只能作为 Profile Version 的内嵌规则配置。

## 7. 后端和前端改动

- Platform API 增加 datasets、evaluation profiles、evaluations 和 reports 模块。
- Evaluation orchestration 只依赖 `EvaluationRuntime`，不直接调用 Worker。
- `/datasets` 接入真实列表并增加 Version、Case 编辑、导入和导出。
- `/evaluations` 增加创建、进度、Case Result、失败分布和版本对比。
- Report 页面通过 Execution ID 跳转 Trace，不能复制 Trace 明细形成第二份权威记录。

## 8. 实施任务

| 编号 | 状态 | 依赖 | 交付物 | 验收条件 |
|---|---|---|---|---|
| EVA-001 | done | FND-003、FND-005 | Dataset、Version、Case Schema 和 Content Hash | Case 变化产生新 Version，旧 Version 不可修改 |
| EVA-002 | done | EVA-001 | Case CRUD、批量导入、校验和导出 | 单个坏 Case 可定位行号且不会产生半成品 Version |
| EVA-003 | done | EVA-001 | Evaluation Profile、内嵌规则和不可变 Profile Version | Run 创建后评分规则配置不可漂移 |
| EVA-004 | done | EVA-003 | Exact、Contains、Regex、JSON Schema 规则 Adapter | 固定输入重复评分结果一致 |
| EVA-005 | done | WCP-004、EVA-001–004 | Evaluation Run、Case Result 和 Metric Schema | Run 固定 Workflow、Dataset 和 Profile 三个 Version |
| EVA-006 | done | EVA-005 | `EvaluationRuntime` 与不可用实现 | 无引擎时不创建伪 Case Result 或通过率 |
| EVA-007 | done | EVA-005 | 报告聚合和版本对比查询 | 成功率、成本、耗时和错误支持 Case 级追溯 |
| EVA-008 | done | EVA-001–007 | REST API、OpenAPI 和审计事件 | 契约覆盖 Version、运行状态、取消和报告分页 |
| EVA-009 | done | EVA-008、FND-011 | Dataset、Version、Case 和导入导出页面 | 大批 Case 有分页、校验错误和未保存提示 |
| EVA-010 | done | EVA-007–009 | Evaluation 创建、进度、报告和对比页面 | 未接引擎显示不可用，报告不使用 Mock 指标 |

## 9. 失败、安全和幂等边界

- Dataset Version 发布使用 Content Hash 幂等，不能在生成过程中暴露部分版本。
- 导入先验证到临时结构，全部通过后在事务内生成 Version。
- 评测取消只停止未开始 Case，已完成 Execution 保留。
- 评分规则失败与 Workflow Execution 失败分开记录。
- LLM Judge 和自定义代码必须遵循阶段 10 的资源授权、成本和沙箱限制。
- Case Input、Expected Output 和报告查询均受 Dataset/Workflow 双重数据权限控制。

## 10. 测试

- Dataset Hash、Profile Version 不可变和四种评分规则单元测试。
- CSV/JSONL 导入原子性、重复 Case 和非法 Schema 集成测试。
- Evaluation Run 状态、取消和 Version 固定契约测试。
- 报告聚合使用固定 Fixture 验证成本、耗时和错误分布。
- 前端 Case 编辑、导入错误、空报告和权限不足测试。

## 11. 验收门禁

- Dataset Version、Profile Version 和评分规则均不可变且可复现。
- Evaluation Run 固定 Workflow Version 和 Dataset Version。
- Report Schema 支持成功率、成本、耗时、工具错误和 Trace 链接。
- Dataset 和 Evaluation 页面不再使用 Mock。
- 引擎未接入时不产生虚假报告。

## 12. 对后续阶段的稳定输出

- Dataset Version、Test Case、Evaluation Profile Version 和评分规则契约。
- Evaluation Run、Case Result 和报告结构。
- `EvaluationRuntime` 批量执行接入点。
- Case Result 到 Execution/Trace 的引用方式。

## 13. 安全删除补充

- Dataset 被任意 Evaluation Run 固定的 Dataset Version 引用时不可删除；无引用时 Case 和 Version 随 Dataset 聚合删除。
- Evaluation Profile 被任意 Evaluation Run 固定的 Profile Version 引用时不可删除；无引用时 Rule 和 Version 随 Profile 聚合删除。
- Evaluation Run、Case Result 和 Report 是不可变历史，不提供物理删除入口。Dataset 与 Profile 分别使用 `dataset:delete` 和 `evaluation_profile:delete`。
