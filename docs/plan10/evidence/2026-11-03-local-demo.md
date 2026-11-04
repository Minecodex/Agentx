# 本地升级与批量报销体验

更新日期：2026-11-03。状态：常驻升级和可独立执行的接口验收通过，真实 API 批次已保留；员工页面仍为草稿，发布和真实浏览器业务验收尚未完成。

## 已部署与验证

本地常驻环境已使用当前源码构建的 11 个服务镜像，标签 `design-browser-20261103-09`，源码标识 `05750ea023a91ba30cd4b8cf76c4da1660e1538b69b528c324eaebc5140c5cdf`。四个服务域的 Doctor 通过，运行中的镜像与源码标识验证通过。

通过现有 Rust 初始化器建立当前数据库基线，备份并保留原数据库，逐行校验后保留 Control 123 表和 Runtime 118 表的数据。账号、原费用报销 Workflow、应用、MCP 和历史记录保留。

本轮已通过的检查：

- 常驻镜像身份和数据保留 pytest。
- 新应用与原单例 Workflow 的输入、输出、资料等待契约一致，页面 SDK 为 2。
- Rust CSV 解析正确处理带逗号的引号字段、金额和物理行号。
- 第三行金额错误导致整批拒绝，批次记录数量不变，未产生正式任务。
- 页面 JavaScript 与离线 DOM 验证、Python 检查、浏览器测试 TypeScript 类型检查。
- 两个 API 体验批次共六项：三项结束（批准两项、拒绝一项）；另三项分别为已完成、等待补充资料、等待审批。每项复用原 Workflow、原任务和原审批。
- 批次请求同键重放不新增对象，同键更换内容被拒绝；项目分页、状态筛选、管理与本人视图一致，跨应用读取被拒绝，待办不能直接派生重做。
- 两个取消验证批次共四项：覆盖未派发取消，以及已受理任务在停止后保持原任务、取消后关闭原资料等待。
- CSV 导出后台生成、幂等重放、大小和 SHA-256 校验、管理端授权下载、未登录下载拒绝、公式触发文本转义。
- 已归属用户的 TSV 文件上传和解析保留物理行号，上传及解析不创建业务任务。

本地交付 pytest 共六项通过：部署与保留数据一项，契约/整批拒绝一项，正式 API 业务四项。离线 DOM 检查验证行标识、一次提交一个批次、准确输入、真实计数字段、输入等待表单保留和导入不提交。离线验证没有启动浏览器，也不作为页面发布检查的替代。

## 直接查看草稿

| 对象 | 入口 |
| --- | --- |
| 批量页面草稿 | [费用报销 · 批量体验 → 访问界面](https://agentx.localhost/applications/01a1a1d2-a3a7-7472-a20f-35f745768800?tab=ui) |
| 批次列表 | [费用报销 · 批量体验 → 调用记录 → 批次记录](https://agentx.localhost/applications/01a1a1d2-a3a7-7472-a20f-35f745768800?tab=invocations&recordTab=batch) |
| 已完成批次 | [三项已结束：两项批准、一项拒绝](https://agentx.localhost/applications/01a1a1d2-a3a7-7472-a20f-35f745768800?tab=invocations&recordTab=batch&batchId=cf458e60-e04f-518f-8858-468b98d804ee) |
| 待办理批次 | [一项完成、一项待补资料、一项待审批](https://agentx.localhost/applications/01a1a1d2-a3a7-7472-a20f-35f745768800?tab=invocations&recordTab=batch&batchId=4213408d-0b65-3813-a742-3221cf025ac0) |
| 待审批示例 | [经理审批原报销任务](https://agentx.localhost/approvals/01a1a214-6101-77b3-a607-acc26d1f8cef) |
| 复用的工作流 | [费用报销 · 网页与 MCP 示例](https://agentx.localhost/workflows/01a19aea-bff1-75f0-a8dd-0ca2dc35346f/versions/01a19aea-bff6-7483-8ce1-7be1381c1ec1) |
| 原单条业务页 | [费用报销服务台](https://agentx.localhost/a/expense-demo) |

新应用绑定原工作流版本，每行仍独立经过申请受理、补充资料、经理审批和结果返回。页面沿用原报销 HTML，增加批量行编辑、CSV/TSV 显式列映射、真实计数、分页、原任务与审批入口、停止、取消、安全派生重做、导出和本人批次记录。本轮未调用付费模型。

新员工入口拟为 `/a/expense-batch-demo`，目前尚未发布，直接打开会返回 404。页面必须完成真实静态及隔离预览检查后发布，不预先写入通过报告。

当前四个正式批次均为 API 来源，合计十项。API 批次创建时没有已发布的页面，故不具备原始页面绑定；管理端可以查看记录、原执行和审批，不能把后续页面伪装成这些批次的原页面。草稿对这类历史条目显示原因，避免提供必然失败的打开动作。员工页面发布后的新网页批次将绑定它自己的发布版本。

原审批拒绝分支会正常结束 Workflow，因此示例中的“审批拒绝”仍计入已完成，不属于系统执行失败。本轮没有为原 Workflow 人为加入故障节点，未声明验证了真实失败后的重做成功。

## 待完成的本地交付检查

`tests/e2e/infrastructure/test_local_batch_delivery.py` 负责本地编排，`tests/browser/tests/local-batch-delivery.spec.ts` 已准备真实页面检查、发布、两批六项、CSV 导入导出、资料补充、批准和拒绝、历史重开、员工明细及管理列表验证。该浏览器测试尚未运行，不能作为已通过证据。

正式接口验收由 `pytest tests/e2e/infrastructure/test_local_batch_api_delivery.py` 编排，没有使用浏览器、替代浏览器或网站权限规避路径。原账本保持 279/377 次、已结算 0.51816372 美元和预留 0.05554368 美元，本轮付费模型请求为零。

当前阻塞来自桌面浏览器工具返回的“已保存权限或管理策略拒绝”。用户已在“网站权限”设置添加规则，但工具仍拒绝访问，尚未确定限制来源。本轮按用户要求完成其余工作，不继续要求用户反复调整权限。

## 用户参与步骤

1. 刷新上面的页面草稿，确保载入最新源码，进入“检查结果”并点击“重新检查”。
2. 检查通过后，点击“发布”，访问后缀填写 `expense-batch-demo`。检查失败时，把报告错误文字发回处理。
3. 发布成功后从访问界面打开员工页面，体验新网页批次。真实浏览器点击、视觉及刷新历史检查仍需正常获得工具授权，或由用户手动完成；当前不标为通过。

本地证据位于 `.local/artifacts/plan10-local-demo-20261103T141604Z/`，状态入口为 `.local/artifacts/plan10-local-demo-active.json`。正式接口报告为 `batch-api-business-verification.json`、`batch-api-cancellation-verification.json`、`batch-api-accepted-cancellation-verification.json` 和 `batch-tsv-file-verification.json`；CSV 样例为 `expense-batch-api-completed.csv` 和 `expense-batch-api-cancelled.csv`。服务及示例保留运行，临时登录文件在本轮结束前清理，原账号密码保持不变。
