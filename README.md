# Agentx

<div align="center">

![版本](https://img.shields.io/badge/版本-v0.0.5--beta-6d5dfc)
![许可证](https://img.shields.io/badge/许可证-Apache--2.0-blue)
![Rust](https://img.shields.io/badge/Rust-1.97.1-orange)
![agentxctl](https://img.shields.io/badge/CLI-agentxctl-orange)
![Helm](https://img.shields.io/badge/部署-Helm-0f1689)
![Kubernetes](https://img.shields.io/badge/运行-Kubernetes-326ce5)

**面向企业场景的开源 Agent 工作流平台**

[快速开始](#快速部署-docker-hub-beta) · [文档](#文档) · [架构](#架构) · [贡献指南](#贡献)

</div>

---

## 概述

Agentx 是一个面向企业场景的开源 Agent 工作流平台，提供可视化 Workflow 编排、模型与 MCP 资源管理、在线调试、应用发布、执行追踪、审批恢复和运行治理。

> **⚠️ 开发阶段提示**  
> 当前版本为 `v0.0.5-beta`，仍处于快速开发阶段，不保证历史数据和旧协议兼容。未经容量、安全、备份恢复和隔离评审，不建议直接用于生产环境。

## 核心能力

### 🎯 Workflow 5.0
可视化编排、强类型参数、表达式、上下文、分支、循环和子工作流。

![工作流画布与执行追踪](docs/images/工作流.png)

### 🤖 Agent Runtime
模型调用、文本流式输出、多模态输入、工具循环、MCP、Skill、RAG、Memory、代码沙箱与 Artifact。

![MCP 集成](docs/images/mcp.png)

### 🚀 应用交付
不可变版本发布、API Key、Webhook、定时任务、参数测试和多轮会话；支持钉钉、飞书、企业微信渠道入站及工作流结果回复。

![应用发布](docs/images/应用发布.png)

### 📊 运行治理
Execution、审批、等待、Checkpoint、Fork、恢复、成本统计和 Trace；支持规则与 LLM Judge 评测、版本比较、错误分布和节点耗时分析。

![调试与追踪](docs/images/调试.png)

### 🏢 企业资源
部门、用户、运行身份、资源授权、模型服务、凭证、MCP、Skill 和知识资源；资源申请包含完整依赖与跨部门会签，LightRAG 支持文档上传、索引状态和检索。

### plan7 源码与验收范围

`v0.0.5-beta` 包含 plan7 的渠道回复、文本流式、评测洞察、知识库管理及真实服务验收修复。ctl 内嵌同版本 Chart、Schema 和全部 11 个镜像配置；发布后下载包保持固定，测试后续源码时需重新构建并使用对应 Values 部署。升级内容见 [0.0.5-beta 发布说明](docs/releases/agentxctl-v0.0.5-beta.md)。

真实 Kimi 文本/SSE/Judge、CPU Embedding、LightRAG、Mem0、OpenSandbox 功能及权限、故障恢复、桌面界面已有专项验收记录。完整范围和剩余项见 [plan7](docs/plan7/README.md)、[真实服务验收](docs/plan7/evidence/p7-live-provider-acceptance.md) 和 [自动化专项验收](docs/plan7/evidence/p7-automatic-boundary-acceptance.md)。真实 IM 回复、多模态与 RAGFlow 联调，以及生产强隔离、容量和完整发布认证仍按报告保留。

## 架构

Agentx 采用三平面分离架构，每个逻辑域独立部署，通过 Helm 管理生命周期。

| 逻辑域 | Helm Release | 物理 Namespace | 主要组件 |
|---|---|---|---|
| Control | `agentx-control` | `agentx-control` | Web Console、Platform Control、可选 Control MySQL |
| Runtime | `agentx-runtime` | `agentx-runtime` | Gateway、Runtime、Worker、Sandbox Manager、可选 MySQL/Redis |
| Observability | `agentx-observability` | `agentx-runtime` | Observability、可选 ClickHouse |
| Dependencies | `agentx-dependencies` | `agentx-deps` | Egress Gateway、local/test Vault 与 MinIO |

**设计原则**
- 核心 Kubernetes 资源只由 Helm 管理
- Kustomize 仅用于可选 Addon 和临时 E2E Fixture
- OpenSandbox 使用官方独立安装流程，Agentx 只验证和接入 Lifecycle API

详细设计见[产品与架构文档](docs/README.md)和[部署手册](deploy/README.md)。

## 快速部署 Docker Hub Beta

### 前置条件

- 对应平台的 `agentxctl` 单文件二进制
- Helm 3、kubectl 和一个可访问且具有默认 StorageClass 的 Kubernetes 集群
- 集群可以拉取 Docker Hub、`cgr.dev`、ingress-nginx 和基础依赖镜像
- OpenSandbox 已独立安装，并可从 Agentx Runtime Namespace 访问

默认 OpenSandbox 地址为 `http://opensandbox.agentx-deps.svc:8080`。如果使用其他地址或主机上的 OpenSandbox，请先按 [OpenSandbox 接入说明](deploy/opensandbox/README.md) 准备自定义 Values，并在安装时传入 `--values`。

### 安装步骤

当前发布：[agentxctl-v0.0.5-beta](https://github.com/Minecodex/Agentx/releases/tag/agentxctl-v0.0.5-beta)。下载的 `agentxctl` 已包含本版本安装所需的 Chart、Schema 和镜像配置，无需克隆仓库或另行下载安装脚本。

#### Windows x64

[直接下载 `agentxctl-windows-x86_64.exe`](https://github.com/Minecodex/Agentx/releases/download/agentxctl-v0.0.5-beta/agentxctl-windows-x86_64.exe) ([SHA-256](https://github.com/Minecodex/Agentx/releases/download/agentxctl-v0.0.5-beta/agentxctl-windows-x86_64.exe.sha256))

也可以在 PowerShell 中下载、校验并安装：

```powershell
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force "$HOME\Downloads" | Out-Null
Set-Location $HOME\Downloads
$release = 'https://github.com/Minecodex/Agentx/releases/download/agentxctl-v0.0.5-beta'
$asset = 'agentxctl-windows-x86_64.exe'
Invoke-WebRequest "$release/$asset" -OutFile $asset
Invoke-WebRequest "$release/$asset.sha256" -OutFile "$asset.sha256"
$expected = (Get-Content "$asset.sha256").Split()[0]
if ((Get-FileHash $asset -Algorithm SHA256).Hash -ne $expected) { throw 'SHA-256 校验失败' }
.\agentxctl-windows-x86_64.exe install
```

#### Linux x64

[直接下载 `agentxctl-linux-x86_64`](https://github.com/Minecodex/Agentx/releases/download/agentxctl-v0.0.5-beta/agentxctl-linux-x86_64) ([SHA-256](https://github.com/Minecodex/Agentx/releases/download/agentxctl-v0.0.5-beta/agentxctl-linux-x86_64.sha256))

在终端中下载、校验并安装（需要 `curl` 和 `sha256sum`）：

```bash
(
set -eu
mkdir -p ~/Downloads
cd ~/Downloads
release='https://github.com/Minecodex/Agentx/releases/download/agentxctl-v0.0.5-beta'
curl -fL --retry 3 "$release/agentxctl-linux-x86_64" -o agentxctl-linux-x86_64 &&
curl -fL --retry 3 "$release/agentxctl-linux-x86_64.sha256" -o agentxctl-linux-x86_64.sha256 &&
sha256sum --check agentxctl-linux-x86_64.sha256 &&
chmod +x agentxctl-linux-x86_64 &&
./agentxctl-linux-x86_64 install
)
```

### 安装说明

不传 `--values` 时，单文件二进制使用与当前 CLI 版本绑定的内嵌 Docker Hub Beta 配置。

`agentxctl 0.0.5-beta` 默认安装全部 `kakj/agentx-*:v0.0.5-beta` 镜像。后续升级请从本页或 [Releases](https://github.com/Minecodex/Agentx/releases) 下载新版 ctl，完成校验后执行 `upgrade`；旧 ctl 的内嵌镜像版本不会自动变化。自定义 `--values` 的用户需要同步更新其中的镜像版本。

安装命令会完成：
1. Values 校验
2. 工具检查（Helm、kubectl）
3. 集群连接测试
4. 三个 Namespace 创建
5. Secret 生成
6. ingress-nginx 安装
7. 四个 Helm Release 安装
8. MySQL/ClickHouse Migration
9. Bootstrap 初始化
10. Rollout 状态检查
11. Helm Doctor 健康检查

任一步失败都会返回非零退出码，Helm 使用 `--atomic --wait --wait-for-jobs` 回滚本次失败发布。

### 验证与访问

查看状态和运行 Doctor：

```bash
./agentxctl-linux-x86_64 status --output json
./agentxctl-linux-x86_64 doctor
```

Windows 使用相同子命令，将二进制名称替换为 `.\agentxctl-windows-x86_64.exe` 即可。

通过端口转发访问 Web Console、完成公司初始化：

```bash
kubectl -n agentx-control port-forward service/web-console 18081:8080
```

打开 `http://127.0.0.1:18081`，按页面提示完成公司初始化。应用调试和对话还需要浏览器能访问部署配置中的 Runtime 地址；仅转发门户不能验证完整执行链路。

本地 HTTPS 部署使用 `https://agentx.localhost` 与 `https://run.agentx.localhost`。两个域名都需要解析到可达入口，并信任 Ingress 所用 CA；门户能打开时，Runtime 请求仍可能被浏览器单独拦截。Docker Desktop 双网络节点的本地方案见 [本地 Ingress Addon](deploy/kustomize/addons/local-ingress/README.md)，实际检查结果见 [HTTPS 入口检查](docs/plan7/evidence/p7-local-https-entry-check.md)。

### 卸载

普通卸载保留 Namespace、PVC 和外部资源：

```bash
./agentxctl-linux-x86_64 uninstall --target all
```

如需同时删除 local/test 的三个 Namespace 和持久化数据，必须显式确认：

```bash
./agentxctl-linux-x86_64 uninstall --target all --purge-data --yes
```

⚠️ production Values 会直接拒绝 `--purge-data`。执行清理前请确认不再需要 PVC 中的数据。

### 自定义部署

自定义或 production 部署必须显式提供 `--values <文件>`。从 Release 下载并解压完整归档包，在解压目录中按实际环境修改 `values/dockerhub-beta.yaml` 或 `values/production.example.yaml`，再执行：

```bash
./agentxctl install --values values/production.example.yaml
```

## 已发布镜像

当前 Beta 镜像均为 Linux AMD64，标签为 `v0.0.5-beta`。

| 镜像 | 用途 |
|---|---|
| `kakj/agentx-web-console:v0.0.5-beta` | Web 管理控制台 |
| `kakj/agentx-platform-control:v0.0.5-beta` | Control API、发布和投影 |
| `kakj/agentx-runtime-gateway:v0.0.5-beta` | 应用调用和 Runtime 查询入口 |
| `kakj/agentx-workflow-runtime:v0.0.5-beta` | 调度、恢复和后台角色 |
| `kakj/agentx-workflow-worker:v0.0.5-beta` | 节点与 Agent 执行 |
| `kakj/agentx-sandbox-manager:v0.0.5-beta` | OpenSandbox 生命周期适配 |
| `kakj/agentx-egress-gateway:v0.0.5-beta` | 受控公网出口 |
| `kakj/agentx-observability:v0.0.5-beta` | Trace 摄取和查询 |
| `kakj/agentx-migrate:v0.0.5-beta` | MySQL/ClickHouse Migration |
| `kakj/agentx-bootstrap:v0.0.5-beta` | 幂等初始化检查 |
| `kakj/agentx-doctor:v0.0.5-beta` | 部署后依赖与权限检查 |

## 本地开发与测试

### 环境要求

- Rust 工具链由 `rust-toolchain.toml` 固定（当前为 1.97.1，Edition 2024）
- Node.js 24
- pnpm 11
- Docker
- Kubernetes 集群（用于集成测试）

### 开发流程

```bash
# 获取源码
git clone https://github.com/Minecodex/Agentx.git
cd Agentx

# 启用 corepack 并安装依赖
corepack enable
pnpm install --frozen-lockfile

# Python 依赖和代码检查
uv sync --frozen --group test
uv run --frozen --group test ruff check .
uv run --frozen --group test pytest tests/acceptance

# Rust 测试
cargo test --workspace

# 前端测试和构建
pnpm --filter @agentx/web test
pnpm build:web
```

### 统一门禁

```bash
cargo xtask check
```

快速检查（跳过 Clippy 和完整测试）：

```bash
cargo xtask check --fast
```

### 本地镜像构建

本地构建并导入 Kubernetes 镜像：

```bash
cargo xtask images --values deploy/values/local.yaml
```

仅构建特定服务：

```bash
cargo xtask images --values deploy/values/local.yaml --service platform-control --service web-console
```

### E2E 测试

领域 E2E 使用临时 Namespace；浏览器场景由 TypeScript Playwright 执行：

GitHub CI 在托管 Ubuntu 上由 pytest 自动创建 Minikube / Calico 集群，Linux 集群 E2E 是必需检查；Windows 保留 ctl 原生构建、测试和打包。Release 使用相同环境完成完整九领域、两小时容量与镜像签名门禁，通过后才公开发布。配置和手动重跑说明见 [CI 说明](docs/ci.md)。

```bash
# 基础设施测试
uv run --frozen --group test pytest tests/e2e --values deploy/values/local.yaml -m infrastructure

# 产品功能测试
uv run --frozen --group test pytest tests/e2e --values deploy/values/local.yaml -m product
```

## 文档

- [产品与架构索引](docs/README.md)
- [系统架构](docs/02-system-architecture.md)
- [Kubernetes 部署契约](docs/07-deployment.md)
- [部署与运维手册](deploy/README.md)
- [E2E 测试规范](docs/plan/e2e-testing-standard.md)
- [OpenSandbox 接入](deploy/opensandbox/README.md)
- [plan7 状态、验收证据与手动测试流程](docs/plan7/README.md)
- [CPU Embedding 测试 Addon](deploy/kustomize/addons/cpu-embedding/README.md)
- [Docker Desktop 本地 Ingress](deploy/kustomize/addons/local-ingress/README.md)

## 贡献

欢迎贡献代码、报告问题或提出建议。开发时遵循 [AGENTS.md](AGENTS.md)，完成相关测试后向 `master` 提交 Pull Request；主分支受保护，需通过必选 CI 检查并解决审查意见后合并。验证命令见[本地开发与测试](#本地开发与测试)，合并规则见 [CI 说明](docs/ci.md)。ctl 发布包和镜像在合入主分支后按发布流程构建、校验和推送。

## 许可证

项目使用 [Apache License 2.0](LICENSE)。

---

<div align="center">

**Made with ❤️ by the Agentx Team**

</div>
