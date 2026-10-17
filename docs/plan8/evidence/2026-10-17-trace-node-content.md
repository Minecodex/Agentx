# Trace 节点归属与内容展示回归

本次修正高级瀑布中的孤立 Runtime Call、节点 Markdown 未渲染，以及节点详情的输入输出布局。没有增加历史数据迁移或前端推断父节点的兼容逻辑。

## 原因与实现

- Runtime Call 原先用 `call_index / 2` 推断轮次，并把父实体生成为 `iteration-<index>`；Agent Core 实际发出的轮次实体使用 `turn-<turn>`。不同 ID 让瀑布找不到父节点，模型、MCP、检索、记忆和沙箱调用因而成为根级记录。现在由同步事件接收器共享实际活动轮次，在预留调用时冻结，并共用一个轮次 ID 生成函数；轮次外调用归属于 Agent Run。取消事件保留相同的父节点与关联身份。
- Agent 生命周期原先在整次运行结束后批量发出，开始和结束时间被压缩成最后的写入耗时。现在在事件发生时写入既有 Trace Outbox；Trace 写入失败仍不改变业务结果。
- 节点语义字符串原先使用普通 `span` 展示。现在复用共享 `MarkdownContent`，支持标题、列表、强调、代码与 GFM 表格，并保留禁用原始 HTML 的规则。正文使用普通前景色。
- “输入与解析参数”和“语义输出”改为上下排列、分别默认折叠；解析参数继续单独展开。JSON 使用统一主题着色、缩进引导线、字段数量和可折叠对象/数组，嵌套内容只在展开时挂载。原始 JSON 中的字符串保持字面值。

共享实现同时用于执行详情、设计器运行轨道与节点 Inspector。现有 Artifact 下载动作和执行范围授权继续使用原路径。

## 验证

| 验证 | 结果 |
| --- | --- |
| 前端完整回归 | 101 文件 / 498 项通过 |
| Runtime 单元测试 | 170 项通过；2 个既有渠道凭证探针忽略 |
| 类型、构建与检查 | Web TypeScript/Vite、前端 lint、浏览器 TypeScript、Rust 格式与 Clippy、Python Ruff 通过 |
| 测试基础设施契约 | 20 项通过；覆盖普通容器、初始化容器镜像覆盖的恢复，以及缩容中断回滚 |
| 临时 Kubernetes 首轮业务回归 | `trace-node-1017b`：1 个 pytest 场景及 3 个 Playwright 场景通过；17 个 Span 无缺失父节点，3 个实际 Runtime Call 的父链与时间范围正确 |
| 最终降级/恢复回归 | `trace-node-1017c`：1 个 pytest 场景、5 个浏览器场景通过，无失败或跳过；覆盖 ClickHouse 停用、权威节点数据保留及恢复后的高级瀑布 |

浏览器通过真实点击验证中文浅色、英文深色、Markdown 标题/表格/强调/代码、输入输出独立折叠、解析参数展开、JSON 逐层展开，以及高级瀑布的节点折叠和搜索祖先保留。

测试入口为 `pytest tests/e2e/product/test_trace_presentation.py`，使用本地 Helm 3、`AGENTX_E2E_RAGFLOW_DISABLE=1` 和现有 `AGENTX_E2E_MEM0_IMAGE` 固定测试依赖。该场景只调用模型和 MCP，不执行 RAGFlow 业务，未跳过任何被测场景。其余应用服务复用既有镜像；新构建的 Worker、Web 镜像指纹与当前源码一致。

首轮安装因错误复用镜像标签中断，失败记录保留在 `trace-node-1017a`。后续补全全部镜像摘要，并明确 Echo 与 Mem0 测试依赖；不以首次安装结果作为产品验收。恢复过程中发现测试基础设施未保存初始化容器的镜像覆盖，已补齐；记忆服务的初始化最终使用原镜像完成，不能把其启动等待全部归因于该缺口。

## 证据与交付边界

- 本地检查与镜像：`.local/artifacts/trace-node-20261017/`，以及 `.local/artifacts/trace-node-regression-*.log`。
- Kubernetes 证据：`.local/artifacts/e2e/trace-node-1017b/`、`.local/artifacts/e2e/trace-node-1017c/`。
- 浏览器截图与 Trace：`.local/artifacts/playwright/helm-agentxctl/trace-node-1017b/`、对应的 `trace-node-1017c/`。

最终三个测试命名空间已删除，镜像加载 Pod 无残留；12 个常驻工作负载恢复到测试前的副本数且全部就绪。最终汇总为 `.local/artifacts/trace-node-20261017/verification-summary.json`，系统 JUnit 为 `e2e-c.xml`。

## 本地常驻发布

已重建并发布全部 11 个产品镜像，版本为 `local-trace-node-release-20261017-220451`。镜像均按摘要固定，源码指纹为 `7e7e6262ac5195ff5714d261b2f40003d76914d81d5b26bd1ce2796e8fb8519c`，与上述 Trace E2E 验证的源码一致。

发布沿用现有 `agentx-prod-control`、`agentx-prod-runtime`、`agentx-prod-deps` 配置，通过 `agentxctl validate --cluster`、渲染检查和全 Target 滚动升级。8 个产品 Deployment 已切换为新镜像，22 个既有工作负载均达到原有预期副本数；3 个迁移任务和 4 个 Doctor 任务完成，镜像加载 Pod 已清理。

通过现有本地 CA 验证 `https://agentx.localhost`，首页和就绪接口返回 200；17 个 JS/CSS 资源的 SHA-256 与运行中 Web 容器一致，其中包含新的 Trace JSON 渲染组件。内置浏览器未信任本地 CA，因此本轮常驻发布采用显式 CA 的 HTTPS 与容器资源核对；交互行为由同一源码指纹的前述浏览器 E2E 覆盖。

发布证据位于 `.local/artifacts/trace-node-release-20261017-220451/`，汇总为 `final-health.json`，镜像摘要为 `images.json`。刷新页面即可使用 Markdown 和上下折叠布局；修正后的 Trace 父子关系和生命周期时间从新执行开始生效。
