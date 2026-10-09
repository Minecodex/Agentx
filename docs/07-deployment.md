# Kubernetes 部署、扩展与可靠性

## 1. 部署事实来源

Agentx 核心 Kubernetes 资源以 Helm 为唯一事实来源。部署主机通过 Rust原生 `agentxctl`运行，Windows 与 Linux 使用相同参数和行为。CLI嵌入固定版本Chart/Schema，调用 Helm/kubectl并解析 JSON输出，不实现第二套 Kubernetes Client，也不下载前置工具。

四个逻辑域对应四个独立 Release：

| Plane | Release | Namespace |
|---|---|---|
| Dependencies | `agentx-dependencies` | Dependencies Namespace |
| Control | `agentx-control` | Control Namespace |
| Runtime | `agentx-runtime` | Runtime Namespace |
| Observability | `agentx-observability` | Runtime Namespace |

ingress-nginx 使用固定上游 Chart和独立 Release `agentx-ingress-nginx`，位于 Dependencies Namespace。Observability 虽与 Runtime 共用 Namespace，仍独立拥有 ServiceAccount、Secret、NetworkPolicy、Migration、Doctor 和 Helm历史。

本地正式部署使用 ingress-nginx 的 LoadBalancer Service，在 Docker Desktop 提供 `http://agentx.localhost` 与 `http://run.agentx.localhost` 入口。`test` 环境及带 Run ID 的临时 E2E 部署使用 ClusterIP，并由测试进程 port-forward，避免争用正式环境的 80/443 端口。此差异只决定入口暴露方式，不改变三平面依赖边界。

开发和生产统一使用 `agentxctl validate/install/upgrade/doctor/uninstall`，核心资源和依赖初始化都由现有四个 Helm Release 管理。Python 只编排构建和 E2E，不另行安装 Agentx 数据库、生成业务 Secret 或创建第二套核心资源。`local-tls.yaml` 在 local 环境验证 TLS 依赖；生产仍只接受预置外部基础设施和 Secret。

本地可显式配置 `global.ingress.serviceType: NodePort` 及不同的 `httpNodePort`、`httpsNodePort`；ctl 返回带端口的访问地址，Ingress Host 必须解析到可达节点。默认 LoadBalancer 不变。production 要求 LoadBalancer，Sandbox 入口额外要求内部 LoadBalancer Annotation；Run ID 只在 local/test 使用，临时 Sandbox 入口使用独立 NodePort。

Kustomize 只管理 LightRAG/Mem0 Addon与临时 E2E Fixture。核心 Helm与 Kustomize资源不得重名、使用同一 Selector或声明相同 Helm所有权。

## 2. Values 契约

环境配置是 YAML，顶层固定为 `global`、`control`、`runtime`、`observability`、`dependencies`。CLI 内嵌与版本绑定的 Docker Hub Beta Values，常规命令省略 `--values` 时使用该配置完成单文件快速部署；自定义和 production 配置必须显式提供。四个 Chart携带相同 `values.schema.json`，测试保证公共定义无漂移。

`global` 组合：

- 三个物理 Namespace和两个 Ingress Host；
- Registry、Repository Prefix、Tag/Digest、Pull Policy和可选 Pull Secret；
- bundled/external MySQL、Redis、ClickHouse、S3、Vault和外部 OpenSandbox；
- Egress Gateway端口、Sandbox私有入口、固定外部依赖 CIDR；
- 权威 Secret、工作负载 Secret和备份 RPO/RTO。

production 必须使用镜像摘要、existing Kubernetes Secret、外部状态依赖、HTTPS/私有 CA、MySQL `verify_identity`、`rediss://` 和受支持的内部 LoadBalancer 配置。外部资源在安装、升级和卸载中都不被创建、修改或删除。

裸金属或本地集群的生产 Profile 可使用平台独立安装的 MetalLB。Sandbox Service 必须同时指定 `loadBalancerClass`、`metallb.io/address-pool` 与单个私网 `metallb.io/loadBalancerIPs`，Endpoint 必须匹配该 IP；ctl 将 Class 与 Annotation 渲染到 Service，集群平台负责提供对应私网地址池及路由。Ingress 通过 `global.ingress.loadBalancerClass` 选择平台控制器；MetalLB 安装的 Class 必须与这些字段一致，避免多个 LB 控制器争用 Service。Class 是 Kubernetes 不可变字段，已有 Service 切换控制器时须先重建。生产仍要求 LoadBalancer、TLS 和来源 CIDR，不使用 NodePort 例外。参考 [MetalLB 地址池配置](https://metallb.io/configuration/_advanced_ipaddresspool_configuration/) 与 [LoadBalancerClass](https://metallb.io/installation/#setting-the-loadbalancer-class)。

Docker Desktop 的单网络节点可由原生 LoadBalancer 提供宿主机 80/443。Docker OpenSandbox 使用 `networkPolicy` 时必须保持默认 `bridge` 网络；其私网 LB 必须在该网段可达。平台把 kind 节点接入 bridge 时，新增网络的 Gateway Priority 必须低于 kind，保留节点默认路由。当前 Docker Desktop 原生 Cloud Provider 无法解析这种双网络节点，Ingress 改用 MetalLB Class，并由固定 Envoy TCP 入口转发宿主机回环 80/443 到 Ingress VIP；具体平台配置见 [本地 Ingress Addon](../deploy/kustomize/addons/local-ingress/README.md)。这是独立平台网络准备，不改变 ctl 的四个 Release 或 OpenSandbox 的默认拒绝策略。

外部 ClickHouse 必须预先创建配置的 Database、迁移账户及 Query/Consumer SQL 用户，并在持久化目录启用 `user_directories.local_directory`。Query/Consumer 不能只定义在只读 `users_xml`，因为 ctl 的 Schema 迁移需要向这些账户授予对应表的权限；迁移账户由平台管理并具备这些授权权限。

## 3. 安装和发布顺序

安装顺序是架构契约：

1. Values/工具/集群/Namespace/生产门禁。
2. production 只读校验所有权威、工作负载、CA/TLS和镜像 Secret，缺项时在创建资源前失败。
3. 创建或验证所选 Target的 Namespace及 Pod Security标签。
4. local/test创建或复用权威 Secret，并发布最小镜像 Secret。
5. 安装 ingress-nginx。
6. 安装 Dependencies并等待 Egress Gateway、Vault、MinIO等适用依赖 Ready。
7. 安装 Control、Runtime、Observability。
8. 等待 Migration、Bootstrap、Init Container、Deployment、StatefulSet和 PDB。
9. 执行每个 Release的 Helm Test/Doctor。
10. 输出 Revision、镜像、Namespace和访问入口；production保存 Release Manifest。

所有 Helm发布使用 `--atomic --wait --wait-for-jobs`。单 Target操作只检查前置 Release，不隐式修改其他 Target。Upgrade从集群读取当前 Deployment副本并通过本次 Helm Override保留；扩缩容必须由操作者显式执行。

## 4. Migration、Bootstrap 与契约窗口

Migration Job是普通 Helm资源，名称包含 Release Revision，不使用安装前 Hook。Job等待数据库并使用数据库锁保证并发唯一执行。应用 Pod的 Init Container查询目标 Schema Version，Schema可用前不启动业务容器。

Bootstrap在首次安装每个 Release时只创建一次，Migration完成前由同一 Job有界重试。Bootstrap必须幂等，双重执行作为 E2E不变量验证。

Expand Migration可手工创建一次性 Revision外 Job。Contract门禁检查旧 ReplicaSet、协议兼容窗口和 Release状态；任何失败都中止发布。Rollback只接受单 Target和明确 Helm Revision，不伪造跨 Release的“整体 Revision”。

## 5. Secret 权威源

Dependencies Namespace中的 `agentx-dependencies-secrets` 是共享 JWT、Bundle、Work Package、User签名、Egress Key/KID、Egress TLS和 Observability Redis材料的权威源。Pod不能跨 Namespace引用 Secret，因此每个工作负载使用本 Namespace的最小镜像 Secret。

- local/test：Rust共享密钥材料库生成RSA、Ed25519和TLS材料；先查权威 Secret，重复 Install/Upgrade保持原值。
- production：只接受预先创建的权威、工作负载和外部依赖 Secret。
- 外部 Vault 的 Control/Runtime 服务 Token 必须已经在 Vault 签发并绑定对应的最小权限策略；只向 Kubernetes Secret 写入随机字符串不会创建 Vault 身份。`CONTROL_VAULT_TOKEN` 与 `RUNTIME_VAULT_TOKEN` 是权威值，工作负载中的镜像值必须一致；更换 Token 后按既有 `sync-secrets` 同步并滚动消费者。Token 的续期、轮换和到期时间由外部 Vault 运维管理，不能将其当作永久密码。签发与生命周期接口见 [Vault Token API](https://developer.hashicorp.com/vault/api-docs/auth/token)。
- Helm：只渲染 Secret名称和 Key，不生成或承载明文。
- 普通 Install/Upgrade：不隐式轮换持久密钥。
- local TLS：Rust 密钥库生成 CA 与每个服务的证书，持久化到权威 Secret，ctl 按平面发布证书和 CA。重复安装/升级复用原材料；改变服务域名时使用新 Namespace 重新签发。Egress 证书和信任锚来自同一权威材料，Ingress 和依赖证书互不覆盖。
- `sync-secrets`：同步与权威源同名的共享 Key并滚动消费者。
- `rotate-egress-keys`：互斥执行双公钥重叠、Gateway Ready、调用方逐个切换、旧公钥删除；失败恢复并重新滚动。

## 6. 外部依赖与 CA

Control、Runtime和 Observability Chart通过投影 Secret把私有 CA只读挂载至 `/etc/agentx-ca`，并设置各 Rust Client的 CA Path。生产不允许关闭证书校验。

TLS 挂载和迁移等待按各组件的 `caSecretName` 配置，和环境名解耦。bundled MySQL 使用挂载密码文件和 `REQUIRE SSL`；Redis 禁用明文监听；ClickHouse、S3、Vault 对客户端提供 HTTPS。本地 Vault 保持开发模式，通过同 Pod 的 TLS 入口访问。独立 OpenSandbox 的本地 HTTPS 入口由 Dependencies Release 的代理提供，`localProxyUpstream` 只允许 local/test；production 直接连接平台提供的 HTTPS OpenSandbox。

Bundled MySQL 的启动和就绪检查使用应用账户在业务 Database 执行 `SELECT 1`，同时验证账户初始化、密码和连接；TLS 配置存在时校验 CA。不能使用认证失败仍返回成功的 `mysqladmin ping` 判断 Ready。Migration 的等待容器保留 MySQL 错误输出，以区分连接、认证和 TLS 失败。

- Control：Control MySQL、S3、Vault CA。
- Runtime：Runtime MySQL、Redis、S3、Vault、OpenSandbox CA。
- Observability：ClickHouse、受限 Redis、S3 CA。
- Sandbox Egress：独立 TLS/CA Secret和单一私有入口。

外部 S3 Bucket必须预创建。PVC不是备份。备份/恢复由外部 Adapter执行，主 CLI只做安全前置、Receipt白名单、RPO/RTO与 JSON Schema验证。

## 7. 网络与安全

- Control/Runtime执行 Restricted Pod Security；Dependencies仅为 ingress/OpenSandbox所需边界放宽。
- Runtime业务 Pod不能直连公网，Model/MCP/Memory/RAG/HTTP等动态流量经 Gateway `3128`。
- Sandbox默认断网；显式允许时只访问 Gateway `3129` TLS入口。
- Gateway应用层和 NetworkPolicy共同拒绝私网、回环、Metadata、保留网段和未批准端口。
- Runtime Gateway在服务层执行浏览器CORS：production只接受配置中的Control Origin，local/test允许临时port-forward Origin；Ingress继续保留同源白名单作为外层防护。
- Observability无 Runtime MySQL凭据，只使用受限 Redis ACL、ClickHouse和独立对象存储身份。
- 核心部署不引入 Prometheus、指标 Adapter、HPA、KEDA、Operator或 GitOps控制器。

严格 NetworkPolicy认证必须在实际执行策略的 CNI上完成；不支持策略执行的本地集群不能形成生产安全证据。

公网 Provider 的 DNS 必须返回真实公网地址。本地代理若把域名解析为 `198.18.0.0/15` 的 fake-IP，production Gateway 会按保留地址策略拒绝 CONNECT，模型连接测试显示 `PROVIDER_UNAVAILABLE`。应在平台 DNS 层修正解析，例如为受影响域名配置 CoreDNS `forward` 到经过证书校验的 DNS-over-TLS 上游，并设置 `tls_servername`；不要通过开放保留网段、关闭 TLS 校验或绕过 Gateway 解决。该 DNS 配置属于集群基础设施，不由 Agentx Helm Release 管理；本地集群重建时需要重新应用。Kimi 实例恢复与真实界面连接测试证据见 [本地模型 DNS 修复](plan7/evidence/p7-kimi-model-connection-fix.md)。

Dependencies Doctor 在所有环境检查 Control 和 Runtime 的 Vault `lookup-self`，使用已配置的 CA 校验证书，任一身份无效即失败。响应体直接丢弃，避免 Token ID/accessor 出现在诊断日志；Token 有效与实际 KV 权限分别由身份检查和凭证业务 E2E 验证。TLS 凭证 UI 创建/轮换及两类失效 Token 的拒绝/恢复验收入口为 `pytest tests/e2e/infrastructure/test_vault_credentials.py`。

production 的 Doctor Pod 通过 `global.network.externalEgress.vault` 中已配置的 CIDR 和端口访问外部 Vault；对应规则只选择 `dependencies-doctor`，保持其他 Dependencies 工作负载的默认拒绝策略。仅允许同 Namespace 的 bundled Vault 会阻断 production 的 Token 检查。

## 8. 健康、扩展和故障恢复

Readiness表示必需依赖与 Schema可用，Liveness只表示进程存活：

- Platform Control无状态扩展，Control MySQL保存权威管理状态。
- Runtime Gateway通过 Runtime MySQL持久化游标，Redis仅唤醒。
- Workflow Runtime多副本使用 MySQL状态条件、Claim/Lease/Fencing和 Outbox。
- Worker按 Capability扩展，并依赖 Runtime MySQL、Redis、S3和 Runtime API。
- `plugin_nodejs` Worker镜像固定Node.js 24.20.0和Runner摘要；启动时校验Node主版本与Runner文件。`pluginMaxProcesses`限制插件消费循环和Node并发数；每次调用独立进程，完成后立即回收。`plugin-work` emptyDir承载调用级文件，源码缓存单独按摘要保留。Linux使用进程组，Windows本地使用Job Object回收整棵进程树。
- Sandbox Manager多副本共享 MySQL Lease并接入独立 OpenSandbox。
- Observability消费受限 Redis Trace Stream写入 ClickHouse；ClickHouse故障不阻断 Execution提交。

Kubernetes重启不能替代幂等、Lease、Fencing、Outbox和恢复逻辑。

## 9. 卸载与数据保护

普通 Uninstall删除对应 Helm Release，但保留 Namespace、PVC和外部资源。Observability卸载不删除 Runtime Release或共享 Namespace。Runtime仍存在时拒绝单独卸载 Dependencies。

只有 local/test、`Target=all`且同时提供 `--purge-data --yes` 时才删除三个 Namespace；production直接拒绝。IngressClass仍有使用者时保留 Controller，Purge模式则失败并要求先处理使用者。

## 10. E2E 和质量门禁

pytest负责临时集群环境、安装/升级/回滚/Doctor/清理、port-forward、日志事件和证据。领域 Marker为 infrastructure、publishing、gateway、runtime、observability、security、upgrade、product。TypeScript Playwright继续负责 UI操作，不改写为 Python浏览器测试；OpenSandbox官方 Go SDK差分 Oracle继续保留 Go。

静态门禁覆盖：ruff、pytest、Values Schema、四 Chart lint/template、Kustomize渲染、资源所有权冲突、表/API/Claim契约、架构边界、2000行限制、Rust/Web测试和 `git diff --check`。运行命令与完整 Runbook见[部署手册](../deploy/README.md)。
