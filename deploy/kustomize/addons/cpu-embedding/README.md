# CPU Embedding Addon

使用 Infinity `0.0.77` 的 OpenAI 兼容 API，在 CPU 运行中文模型
`BAAI/bge-small-zh-v1.5`。输出维度为 512，模型固定为
`7999e1d3359715c523056ef9478215996d62a620`，权重随镜像发布，运行时不下载。
构建时从 ModelScope 下载权重，并校验固定 Hugging Face LFS SHA-256
`354763b9b1357bc9c44f62c6be2276321081ed2567773608c0d0785b61d5a026`。
CPU 使用普通 Torch 推理；Click 固定为 8.1.8，匹配 Infinity 的 Typer 版本。

这个可选 Addon 不属于 Agentx 核心 Helm Release。先准备
`agentx-embedding-secrets` Secret，其中 `API_KEY` 是本地生成的服务凭证，
再按现有 `cargo xtask images --values <local-values> --service cpu-embedding`
构建并导入镜像，通过 `kubectl apply -k` 安装。发布时使用固定镜像摘要。

集群内接口是 `http://cpu-embedding.<namespace>.svc:7997/v1/embeddings`。
LightRAG 的 `EMBEDDING_DIM`、Mem0 的 embedder/vector_store 维度必须都设为
512；LLM 和 embedding 分别使用各自地址与凭证。

服务只接收同 Namespace 的 Provider Pod，不允许出站访问；客户网络策略仅
放行 LightRAG/Mem0 到该服务的 7997 端口。默认请求 250m CPU/512Mi 内存，
限制 2 CPU/2Gi 内存。它用于本地功能验收，不能作为检索质量或生产吞吐认证。

系统测试通过 `pytest tests/e2e` 准备临时 Namespace、验证真实向量与中文
语义排序，并在完成后清理。卸载本 Addon 没有持久数据需要保留。
测试前设置 `AGENTX_E2E_CPU_EMBEDDING_IMAGE` 为镜像完整摘要引用；真实文本、
知识库和 Agent 记忆测试还需设置 `AGENTX_E2E_LIVE_VAULT_SECRET_REF`，读取
本机已配置的 Kimi Vault 凭证。测试不会将模型 key 写入文件或浏览器录制。
