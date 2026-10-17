# 外部知识连接、查询配置与多来源检索

Agentx 只接入、授权和检索外部知识。内容、文档变更、解析与索引由 Provider 维护。

## 管理与授权

连接列表独立创建，不要求资源名称或外部 ID。详情下的每条查询包含名称、用途、范围、所属部门、状态和授权入口。RAGFlow 多选真实 Dataset；LightRAG 绑定实际实例返回的一个 workspace，包含默认空字符串。两种协议都不提供目录过滤。

查询沿用 `rag` 资源及部门、Workflow、依赖审批机制。相同范围可配置多条查询并独立授权。查询名称在连接内唯一。查询被 Workflow/Skill 引用时拒绝删除；连接存在查询时拒绝删除。外部内容不受删除配置影响。

接口：

| 路径（均在 `/api/v1/knowledge` 下） | 操作 |
| --- | --- |
| `/connections` | 列表、创建 |
| `/connections/{id}` | 详情、修改、停用、删除 |
| `/connections/{id}/test-connection` | 使用 Provider 认证测试 |
| `/connections/{id}/catalog` | 读取真实 Dataset/workspace |
| `/connections/{id}/queries` | 分页列表、创建查询 |
| `/queries/{id}` | 查询详情、修改、删除 |
| `/queries/{id}/documents` | 只读外部文档；RAGFlow 使用所选范围内的 datasetId 分页 |
| `/queries/{id}/retrieval-test` | query + topK；不修改配置 |

存储保留 `rag_connections` 与 `rag_resources`。范围为带 `kind` 的结构化 JSON，删除 `external_resource_id/sync_status`。删除本地知识文档表、Artifact 引用与索引任务；直接更改初始化基线。

## 执行

Agent 附件与独立知识节点都支持跨连接多查询。Agent 为每条查询派生工具，以名称与用途供模型选择；独立节点查询所有选择项。包冻结查询版本、连接版本、scope 和凭证版本，外部索引始终使用当前内容。Runtime 的 `runtime_resource_states` 按资源、版本和内容指纹保存状态；不同冻结配置不再争用同一行。状态命令针对明确的版本/指纹，查询级撤销仍通过现有 ResourceGrant 投影统一阻止所有版本。此变更只改变 Runtime 自有表的键，不改变跨面访问边界。

LightRAG 实例在查询前校验真实 workspace，不使用 header 或请求字段切换。RAGFlow 按 Dataset 发请求，原生 `page_size=20` 控制返回候选数量；原生向量候选 `top_k` 与最终 Top K 分开。

共享执行器：先完成授权校验，再合并重复来源，使用 futures 并发 4；每来源取最多 20 候选，RRF 常数 60，最终 Top K 默认 5、1–20。相同连接的相同片段去重，保留所有命中查询及来源。原始分数位于来源记录，融合分数独立输出。

结果包含 `text/documents/files/partial/sources`。来源状态携带查询、连接、Dataset/workspace、耗时与错误。部分成功返回成功来源结果；成功空列表属于成功；全部成功但空命中是正常空结果；全部失败走错误出口并保留来源详情。授权失败不得降级为部分结果。附件是外部引用，不变成平台 Artifact。

Agent 每次知识工具调用也遵循该查询范围的失败规则：部分成功或正常空结果允许模型继续；全部来源失败时，工具结果保留结构化错误并终止当前 Agent，错误出口通过 `details.sources` 提供来源详情。复用 Agent Core 的 `terminate` 机制，终止结果先持久化；Worker 在工具结算与终态写入之间重启，也不能继续调用模型或丢失错误详情。

## 验收

- 契约与单元测试：结构化范围、版本快照、多绑定、凭证依赖、撤销、RRF、重复来源、空结果和失败语义。
- Playwright：桌面连接 CRUD、查询与授权、只读文档、测试弹窗、跨连接选择、中文/英文与浅色/深色。
- 真实服务：多个 RAGFlow Dataset、两个 LightRAG workspace 实例。使用 Provider 原生 API 预置及追加文档，验证范围隔离和外部新增内容可检索。
- 真实模型 Agent：按用途仅调用相关查询，以及一次任务调用跨连接的多个知识工具；检查实际工具操作记录与最终回答。另测部分成功、空结果、全部失败和失败后的模型调用次数。
- 跨部门授权：查询与凭证分属两个部门，两方都批准后才允许检索；覆盖审批隔离、拒绝、重新申请、取消及凭证授权撤销。复用桌面审批测试验证两位审批人的脱敏视图、会签和拒绝，并执行审批后的查询。
- `uv run --frozen --group test pytest tests/e2e` 是环境编排入口。临时 Namespace 与恢复开发服务由现有 fixture 管理，证据位于 `.local/artifacts`。

实现与验收记录见 [2026-10-17 知识连接验收](evidence/2026-10-17-knowledge-connections.md)。

设计参考：[LightRAG 实例模型](https://github.com/HKUDS/LightRAG/blob/main/docs/LightRAG-API-Server.md#starting-multiple-lightrag-instances)、[RRF](https://learn.microsoft.com/en-us/azure/search/hybrid-search-ranking)。
