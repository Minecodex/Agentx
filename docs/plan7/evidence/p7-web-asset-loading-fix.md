# 本地升级后工作流详情页分块加载修复（2026-10-08）

新建工作流后，浏览器请求升级前的 `workflow-detail-page-CgJqAL-D.js`，详情页显示 React Router 默认错误页。实测该文件已不在当前门户镜像中，而 NGINX 将缺失 JS 当作 SPA 路由返回首页：HTTP 200、`Content-Type: text/html`，入口也没有明确的缓存策略。

## 修复与部署

- HTML 入口和 Runtime 配置禁止缓存；带内容摘要的静态资源使用不可变缓存，缺失资源返回不缓存的 404。
- 按 [Vite 官方加载错误处理](https://vite.dev/guide/build#load-error-handling) 监听 `vite:preloadError`，每个构建、每个标签页最多自动刷新一次。持续失败及存储不可用时提供统一、已国际化的路由错误页，支持手动刷新与返回首页。
- 重建 `web-console:plan7-webfix-20261008`，固定镜像摘要 `sha256:9678013f3e25c9049149b01c7398cf47bf51a532f39f0abb404d006d31c13002`。前端修复源码摘要为 `1aecea1999a708a9f7e74c401102b1f04dc8bb306bf545ef2acf7605a03e220f`。
- `agentx-control` 本地 Release 升级到 revision 10，Doctor 通过。只有门户镜像发生变化，后端镜像仍沿用完整候选 `d041bea6…`；现有部署副本保持 Ready。本次没有执行业务数据清理。
- 已加载旧入口的报错标签页需要强制刷新一次，之后可从列表打开已创建的工作流。

## 验证

- 前端构建成功，全量 Vitest：91 个测试文件、408 项通过；相关 Python Ruff 与 Git diff 检查通过。
- 本地 HTTPS 实测：根入口、工作流深链、Runtime 配置为 200 且 `Cache-Control: no-store`；旧分块为 404 且不缓存；当前 `workflow-detail-page-B9GCpVTQ.js` 为 200 JavaScript 且不可变缓存。
- 隔离集群桌面 Playwright：4 项通过，失败、错误、跳过均为 0。
  1. 入口和静态资源缓存、缺失 JS 的真实响应。
  2. 通过可见表单新建工作流，进入详情，再点击进入画布。
  3. 模拟单次详情分块缺失，自动恢复且工作流创建只提交一次。
  4. 模拟持续缺失，不循环刷新；移除故障后通过“刷新页面”恢复。

证据位于 `.local/artifacts/plan7-workflow-chunk-fix-20261008/`；浏览器报告、Trace 和截图位于 `.local/artifacts/playwright/helm-agentxctl/p7webfix1008/web-asset-loading/`。系统级入口为 `pytest tests/e2e/product/test_web_asset_loading.py`，该修复属于门户专项验证，不替代 plan7 全量产品及生产发布认证。
