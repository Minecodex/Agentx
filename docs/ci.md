# PR 与 CI 合并规则

日常开发使用功能分支和 PR。默认分支要求 GitHub Actions 的固定 `CI` 检查、同步最新主分支并解决审查讨论；失败、取消或意外跳过不能通过汇总门禁。禁止强推和删除，不设置管理员绕过。

## 执行层级

| 入口 | 实际范围 |
| --- | --- |
| 普通 PR / master push | Windows/Linux 原生源码、前端 lint/单测/构建；临时 Minikube 中 30 项核心系统测试 |
| quality 手动 full / 版本候选 | 全部受控系统、浏览器、提供商容器、权限、故障及升级回归；排除真实模型与生产容量 |
| full 画布性能 Job | 独立托管 Runner 与全新临时 Minikube，只运行原画布 benchmark，保留原帧率/输入延迟阈值 |
| agentxctl-release | 同一不可变 Tag 的完整托管软件检查；Windows/Linux 候选包构建、解压和原生执行；只保存 Actions 制品 |
| 独立环境 | 真实 Kimi/知识记忆质量、冻结容量矩阵和两小时窗口、公共注册表签名及正式发布认证 |

核心测试清单在 [ci_profiles.py](../tests/e2e/ci_profiles.py)。PR 保留安装、镜像归属、初始化、流式模型、应用会话、渠道投递、Loop 恢复、幂等与鉴权，以及画布插件/API-first/应用文档和资源加载的真实浏览器流程。完整提供商、画布性能、复杂业务组合和全部故障矩阵在托管发布范围执行。清单中的测试缺失或选择为空时直接失败。

当前收集为 PR 30 项、完整受控功能 107 项及独立性能 1 项；默认 independent 保留全部 130 项，包括 22 项真实模型/容量参数化测试。依赖真实 Kimi 凭证的测试必须声明 live_model，遗漏声明在托管收集阶段直接报错。性能 Job 不部署与指标无关的 RAGFlow；仍使用实际安装的 UI/API、实际插件制品和原 benchmark，不在普通 PR 执行。

性能测量关闭视频和逐操作 DOM/截图 trace，避免记录器进入帧与输入延迟样本；功能套件继续记录。每档原始指标在断言前保存，失败档也保留证据；50/100 ms 输入延迟和原帧率阈值不变，没有按失败结果调参或自动重试。

画布的插件契约和变量目录只订阅节点数据、连接、Start/End 和执行设置；拖动、尺寸测量与视口变化不再重新序列化全部插件或重建变量目录。参数修改即使不改变 GraphIndex revision 也会刷新定义；保存、恢复与历史记录仍读取当前完整文档。

系统入口保持 `pytest tests/e2e`，通过 `--ci-profile pr|release|independent` 选择范围。独立验收默认保留全部测试及原有容量阈值；Actions 不提供真实模型密钥，不把环境测试排除报告为通过。Minikube/网络及证据按运行所有权创建和清理。

完整托管软件验证：
```sh
uv run --frozen --group test pytest tests/e2e --minikube --values deploy/values/local.yaml --ci-profile release -m 'not browser_performance'

# 独立的一次托管性能运行；CI 为该步骤另起 Runner/临时集群
uv run --frozen --group test pytest tests/e2e/product/test_canvas_performance.py --minikube --values deploy/values/local.yaml
```

独立环境具备后使用默认 independent 范围、真实临时提供商凭据和实际兼容 Worker 摘要执行既有认证；现有 `tools/scripts/release` 的完整证据、签名与公开发布校验继续保留。Actions 不推送 Docker Hub 镜像、不创建公开 CLI Release，也不依赖专用 Runner、Actions Secret 或 Environment。

隔离的 boundary-echo-mcp 仅加入 local 测试 values 和托管临时集群的 HTTP 提供商服务白名单；生产默认值和 HTTPS/域名校验保持原有要求。使用自定义独立验收 values 时，也需显式列入该测试服务。

`E2E diagnostics` 仅聚焦 PR/受控范围，不产生必需 CI 或正式认证。源码固定在当前候选，截图基线必须显式更新并正常比较复验。包、存储、网络、权限、TLS 与供给链的原有断言不放宽。没有定时任务。

Minikube 显式映射的 NodePort 地址/端口在 agentxctl --run-id 作用域处理后保留，避免被改写为容器中不可解析的 Docker Desktop 地址。当前节点矩阵截图已与实际 Linux UI 对照并显式更新，像素比较保持有效。Redis 恢复等待 Gateway 观察到重连，拒绝期间仍不得产生 Receipt；调用投影等待终态，失败调用仍使测试失败。

边界鉴权使用独立 boundary-echo-mcp Deployment/Service 与随机密钥，不修改其他场景的共享 Echo 凭据。连线测试同时核验规范图中只增加一条连接和指定节点对，不把 ReactFlow 延迟挂载的 SVG 总数当作新增操作的起点；最终四条连接的持久化断言保留。diagnostics 的 performance、attachments、studio 仅用于这些聚焦复验，不替代完整发布门禁。
