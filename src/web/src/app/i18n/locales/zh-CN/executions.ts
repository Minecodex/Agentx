const translations = {
  "draftRevision": "草稿 {{draft}} · r{{revision}}",
  "trace": "查看 Trace",
  "mainViews": { "trace": "Trace", "recovery": "恢复" },
  "cost": "成本",
  "title": "执行记录",
  "description": "查询工作流执行、Trace 和检查点。",
  "search": "搜索执行 ID、Trace ID 或错误码",
  "trigger": "触发方式",
  "executionIdLabel": "执行 ID",
  "startedAt": "开始时间",
  "duration": "耗时",
  "source": "来源",
  "initiator": "发起人",
  "application": "应用",
  "workflow": "工作流",
  "tool": "实际调用工具",
  "user": "用户",
  "department": "发起部门",
  "moreFilters": "更多筛选",
  "clearAll": "清除全部",
  "createdAfter": "开始时间下限",
  "createdBefore": "开始时间上限",
  "dateTimePicker": {
    "time": "时间",
    "now": "现在",
    "clear": "清除",
    "invalidTime": "请输入有效时间，格式为 HH:mm。",
    "outOfRange": "所选时间不在允许范围内。"
  },
  "triggerName": "触发名称",
  "triggerNamePlaceholder": "搜索执行时触发名称",
  "selectApplication": "选择应用",
  "selectWorkflow": "选择工作流",
  "selectTool": "选择工具",
  "selectUser": "选择用户",
  "selectDepartment": "选择部门",
  "selectTriggerType": "选择触发方式",
  "filterLoadFailed": "筛选选项加载失败",
  "resultsRefreshed": "查询游标已过期，结果已刷新",
  "total": "共 {{count}} 条"
  ,"nodePanel": {
    "selectNode": "选择左侧节点查看运行数据", "ariaLabel": "节点运行数据", "outlineAriaLabel": "执行节点大纲", "outline": "执行节点大纲", "runCount": "{{count}} 次运行", "noNodes": "尚无节点运行", "run": "运行", "input": "输入", "output": "输出", "lineage": "Lineage", "attempts": "尝试记录", "logs": "日志", "noInput": "该节点没有输入", "noOutput": "该节点尚无输出", "noLineage": "没有 Lineage 记录", "noAttempts": "没有尝试记录", "noLogs": "没有节点日志", "attempt": "尝试 {{number}}", "worker": "Worker", "deadline": "截止时间", "started": "开始时间", "sourceSummary": "运行 {{run}} · 输出 {{output}} · 项 {{source}} → 项 {{target}}"
  }
  ,"recovery": {
    "ariaLabel": "恢复与事件", "title": "时间线与恢复", "approval": "审批", "sideEffectConfirmation": "副作用确认", "openApproval": "打开审批", "irreversibleWaiting": "不可逆节点正在等待恢复决策。", "handle": "处理", "checkpoints": "检查点 · {{count}}", "noCheckpoints": "尚无检查点", "events": "事件", "noEvents": "尚无时间线事件", "activationsAndDeliveries": "{{activations}} 次激活 · {{deliveries}} 次投递", "download": "下载"
  }
  ,"sideEffects": { "none": "无", "reversible": "可恢复", "irreversible": "不可逆" }
  ,"forkDialog": {
    "title": "派生执行", "description": "预览派生执行将复用、重跑和需要确认的节点", "intro": "原执行保持只读，新执行拥有独立状态与 Trace。", "checkpoint": "检查点", "scope": "执行范围", "targetNode": "目标节点", "inputOverrides": "输入覆盖", "selectCheckpoint": "请选择检查点", "selectNode": "请选择节点", "preview": "执行预览", "previewSummary": "{{rerun}} 个重跑 · {{reuse}} 个复用", "run": "运行", "rerun": "重跑", "reuseOutput": "复用输出", "willExecute": "将执行", "noSideEffect": "无副作用", "decision": "{{name}} 的副作用决策", "dryRun": "预演", "reusePreviousOutput": "复用之前的输出", "confirmExecute": "确认执行", "risk": "{{count}} 个不可逆节点需要明确决策。预演不会调用远程副作用，复用输出不代表重新执行。", "noNodes": "没有可预览节点", "creating": "正在创建…", "create": "创建派生执行", "modes": { "whole": "完整执行", "node": "单节点", "to_node": "执行到节点", "from_node": "从节点继续" }
  }
  ,"sideEffectDialog": { "title": "副作用确认", "description": "选择不可逆节点的恢复策略", "intro": "本次派生执行即将经过不可逆节点。确认执行会再次产生真实副作用；复用之前的输出只复用父执行的结果；预演仅传递带决策标记的输入。", "decision": "副作用决策", "submitting": "正在提交…", "confirm": "确认决策" }
  ,"loadingExecution": "正在加载执行"
  ,"noPermission": "没有查看权限"
  ,"loadFailedTitle": "执行加载失败"
  ,"loadingDescription": "正在读取运行快照、节点和恢复状态。"
  ,"cancelledToast": "执行已取消"
  ,"sideEffectSubmitted": "副作用决策已提交"
  ,"backToList": "返回执行列表"
  ,"executionId": "执行 {{id}}"
  ,"durationLabel": "耗时"
  ,"triggerLabel": "触发方式"
  ,"refresh": "刷新执行"
  ,"cancelExecution": "取消执行"
  ,"forkExecution": "派生执行"
  ,"parentExecution": "父执行 {{id}}"
  ,"callerExecution": "调用方执行 {{id}}"
  ,"return": "返回"
  ,"cancelTitle": "确认取消执行？"
  ,"cancelDescription": "取消会持久化终止状态、释放租约，并使后续恢复失效。此操作不会删除运行历史。"
  ,"executionTypes": {
    "whole": "完整执行",
    "node": "单节点执行",
    "to_node": "执行到节点",
    "from_node": "从节点继续",
    "fork": "派生执行",
    "sub_workflow": "子工作流执行"
  }
  ,"triggerTypes": {
    "manual": "手动",
    "api": "API",
    "application": "应用",
    "webhook": "Webhook",
    "schedule": "定时触发",
    "event": "事件",
    "sub_workflow": "子工作流"
    ,"user": "用户调用"
    ,"api_key": "API Key"
    ,"debug": "调试"
    ,"evaluation": "评测"
    ,"poll": "轮询"
    ,"lifecycle": "生命周期"
    ,"fork": "派生执行"
    ,"composite": "子工作流"
  }
} as const

export default translations
