> 知识库管理与检索部分已由 [连接、查询配置与多来源检索](../plan8/11-knowledge-connections.md) 取代。本页保留历史设计与验收背景。

# 资源补全、独立知识检索与错误出口

状态：done（本次功能范围）。此前 17 个系统场景与发布记录见 [验收报告](evidence/2026-10-16-knowledge-and-resources.md)。真实 LightRAG no_results、用户身份投影恢复与失败执行可见性已修复；错误出口优先复用，并通过 9 个增量系统场景分轮验收及原工作流真实运行，见 [后续修复报告](evidence/2026-10-16-empty-results-and-execution-visibility.md)。容量压测按用户要求 deferred。

## 已确认范围

- AI 按用户需求查询现有资源，优先名称唯一匹配，保存真实 ID/版本；同名歧义需问用户，没有匹配项留空。只绑定相关资源，不批量添加所有可用附件，不代替用户授权。
- 总结逐节点列出已绑定、缺失、待授权、禁用或失效的配置。租户画布创作模型与业务节点模型独立。
- 手动和 AI 草稿均允许缺少资源；资源槽类型、数量、已有引用版本仍严格校验。完整编译/运行/版本创建拒绝缺少必需资源。
- 新公开节点 `knowledge_retrieval` 使用一个已配置 RAG 资源及 query/Top K，复用 Runtime 冻结绑定、凭据、调用账本与 LightRAG/RAGFlow 协议。Agent 知识附件保留，内部 `rag` 执行器不作为画布入口。此项更新 plan5 的仅附件边界。
- 输出 `text`（拼接的命中片段）、`documents`（来源、片段、分数与元数据）、`files`（Provider 已返回的附件来源引用）。不下载原文件，不把外部文档/附件 ID 或 URL 冒充平台 Artifact；无命中返回空文本和空数组，服务失败走 error。
- AI 为新建流程中未处理的错误出口优先复用同作用域现有 Exit.error（包括同时接收成功输出的受保护 End）。检查共享错误契约和各必填字段映射，通用字段引用当前 error item；缺失映射在授权范围内原子补齐，保留已有成功输出和自定义处理。无兼容出口或用户明确要求独立处理时才新增 Exit。错误字段引用当前 error item；Loop 内遵守容器传播及 errorMode，不跨边界接线。节点范围请求不得为补出口越权修改全局契约。
- 编辑事务拒绝时保留结构错误码、字段路径和具体解析原因，供用户与 AI 修正；共享出口契约与所有受影响的 Exit 映射在同一事务提交。非法 Reference 不增加兼容形状。
- Exit 必填映射按实际接入 main/error 端口检查；已有映射无论入口是否接线仍检查字段和引用。错误 Exit 不要求无关的成功映射。
- Builder 仍只有七个设计工具；检索与所有业务测试只能由用户界面运行。

## 参考

[Dify Knowledge Retrieval](https://docs.dify.ai/en/cloud/use-dify/nodes/knowledge-retrieval)：独立检索节点配置 query、知识库和检索设置，结果包含片段及来源；其 files 可承载结果中的图片附件，不等同命中文档原件。

[RAGFlow HTTP API](https://ragflow.io/docs/http_api_reference#retrieve-chunks)：检索结果的 document_keyword、document_metadata 和 image_id 分别保留为来源标题、元数据和已有附件引用；image_id 不拼接为下载 URL。

节点发现支持多词和本地化名称搜索；Exit 作为边界类型返回 Rust 生成的 `ExitParameters` / `WorkflowEnd` Schema 与错误映射示例，明确共享字段声明和每节点 Binding 的区别。

## 验收

- [x] 契约、编译器、Runtime、画布配置和变量引用一致。
- [x] 缺资源可保存，错误类型/重复绑定拒绝，完整编译拒绝缺资源。
- [x] 独立成功/错误 Exit 校验、错误 item 映射及真实失败终态。
- [x] AI 名称匹配、无匹配缺项总结、未授权状态、错误路径补全。
- [x] 浏览器创建/编辑/保存重开知识节点；手动运行命中/无命中/失败，创建期间无业务 Execution。
- [x] 临时 Kubernetes E2E、真实创作模型验证和环境清理。
