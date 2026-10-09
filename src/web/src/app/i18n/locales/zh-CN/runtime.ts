const translations = {
  "runtimeStatus": "运行状态",
  "runtimeDescription": "仅展示工作流业务链路中的 Worker、Queue、沙箱与 Trace 状态。",
  "failedToday": "今日失败",
  "runtimeComponents": "运行组件",
  "components": {
    "runtime": "运行服务",
    "coordinator": "协调器", "worker": "Worker", "sandbox": "沙箱", "trace_writer": "Trace 写入器", "trace_queue": "Trace 队列", "queue": "队列"
  },
  "instances": "实例",
  "noHeartbeat": "无心跳",
  "activeSandboxes": "活跃沙箱",
  "sandboxCompatibility": "OpenSandbox 协议兼容状态",
  "runtimeQuotas": "运行配额",
  "workerCapabilities": "Worker 能力",
  "retention": "数据保留",
  "dryRun": "预览清理",
  "cleanup": "执行清理",
  "cleanupConfirmation": "将按保存期限清理未被引用的 Artifact、Trace、应用消息和评测报告；被版本、检查点、会话或评测对比引用的数据会保留。此操作不可撤销。",
  "governance": {
    "quotaDescription": "限制公司范围内的并发、每日消耗和活跃沙箱资源，防止运行任务占满平台容量。",
    "used": "已使用",
    "reserved": "已预留",
    "capabilityDescription": "显示当前在线 Worker 可以执行的功能类型；同一功能有多个 Worker 时会合并显示。",
    "workerCount": "{{count}} 个 Worker",
    "retentionDescription": "按保存期限清理未被引用的数据：Artifact 30 天、Trace 180 天、应用消息 180 天、评测报告 365 天。预览清理只统计候选项，不会删除。",
    "runStatus": {
      "queued": "等待中",
      "running": "清理中",
      "completed": "已完成",
      "failed": "失败"
    }
  },
  "dashboard": {
    "eyebrow": "工作空间总览",
    "greeting": "早上好，林晓",
    "intro": "这里是 {{tenant}} 今天的工作流运行概况。",
    "newWorkflow": "新建工作流",
    "recent": "最近工作流",
    "metrics": {
      "running": "运行中",
      "today": "今日执行",
      "successRate": "成功率",
      "cost": "今日成本",
      "realtime": "实时",
      "budget": "预算 62%",
      "executions": "个执行"
    }
  },
  "quota": {
    "execution_concurrency": {
      "label": "工作流并发",
      "unit": "个执行",
      "period": "并发容量"
    },
    "node_concurrency": {
      "label": "节点并发",
      "unit": "个节点",
      "period": "并发容量"
    },
    "sandbox_concurrency": {
      "label": "沙箱并发",
      "unit": "个沙箱",
      "period": "并发容量"
    },
    "agent_iterations": {
      "label": "智能体迭代额度",
      "unit": "次",
      "period": "每日"
    },
    "tokens": {
      "label": "Token 额度",
      "unit": "Tokens",
      "period": "每日"
    },
    "cost_micros": {
      "label": "模型成本额度",
      "unit": "货币微单位",
      "period": "每日"
    },
    "artifact_bytes": {
      "label": "Artifact 存储容量",
      "unit": "GB",
      "period": "存储容量"
    },
    "cpu_millis": {
      "label": "沙箱 CPU 容量",
      "unit": "CPU 核",
      "period": "并发容量"
    },
    "memory_bytes": {
      "label": "沙箱内存容量",
      "unit": "GB",
      "period": "并发容量"
    },
    "pids": {
      "label": "沙箱进程容量",
      "unit": "个进程",
      "period": "并发容量"
    },
    "disk_bytes": {
      "label": "沙箱磁盘容量",
      "unit": "GB",
      "period": "并发容量"
    },
    "ttl_seconds": {
      "label": "沙箱时长容量",
      "unit": "小时",
      "period": "并发容量"
    }
  },
  "capability": {
    "plugin_nodejs": "画布插件（Node.js）",
    "agent": "智能体节点",
    "builtin": "内置节点",
    "declarative_http": "HTTP 请求节点",
    "mcp_tool": "MCP 工具",
    "memory": "记忆读写",
    "model": "模型调用",
    "rag": "知识检索",
    "sandbox": "沙箱执行",
    "skill": "技能能力"
  },
  "capabilityStatus": {
    "ready": "可用",
    "unavailable": "不可用",
    "unknown": "未知"
  },
  "retentionDataType": {
    "artifact": "Artifact 文件",
    "trace": "执行 Trace",
    "application_message": "应用消息",
    "evaluation_report": "评测报告"
  },
  "retentionStatus": {
    "candidate": "待清理",
    "deleted": "已清理",
    "blocked": "受保护",
    "failed": "失败",
    "pending": "等待中"
  },
  "retentionReasons": {
    "expired": "已超过保存期限", "referenced": "仍被引用", "legal_hold": "处于保留状态", "deletion_failed": "清理失败"
  }
} as const

export default translations
