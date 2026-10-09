const translations = {
  "runtimeStatus": "Runtime status",
  "runtimeDescription": "Workflow-only status for Workers, Queue, Sandbox, and Trace delivery.",
  "failedToday": "Failed today",
  "runtimeComponents": "Runtime components",
  "components": {
    "runtime": "Runtime service",
    "coordinator": "Coordinator", "worker": "Worker", "sandbox": "Sandbox", "trace_writer": "Trace writer", "trace_queue": "Trace queue", "queue": "Queue"
  },
  "instances": "instances",
  "noHeartbeat": "No heartbeat",
  "activeSandboxes": "Active sandboxes",
  "sandboxCompatibility": "OpenSandbox protocol compatibility",
  "runtimeQuotas": "Runtime quotas",
  "workerCapabilities": "Worker capabilities",
  "retention": "Data retention",
  "dryRun": "Preview cleanup",
  "cleanup": "Run cleanup",
  "cleanupConfirmation": "Unreferenced Artifacts, Traces, application messages, and evaluation reports will be removed by retention period. Data referenced by versions, Checkpoints, sessions, or evaluation comparisons is kept. This cannot be undone.",
  "governance": {
    "quotaDescription": "Limits company-wide concurrency, daily consumption, and active Sandbox resources so workloads cannot exhaust platform capacity.",
    "used": "Used",
    "reserved": "Reserved",
    "capabilityDescription": "Shows the functions currently available from online Workers. Multiple Workers with the same function are grouped.",
    "workerCount": "{{count}} Workers",
    "retentionDescription": "Removes unreferenced data by policy: Artifacts after 30 days, Traces after 180 days, application messages after 180 days, and evaluation reports after 365 days. Preview only counts candidates and never deletes.",
    "runStatus": {
      "queued": "Queued",
      "running": "Cleaning",
      "completed": "Completed",
      "failed": "Failed"
    }
  },
  "dashboard": {
    "eyebrow": "Workspace overview",
    "greeting": "Good morning, Lin Xiao",
    "intro": "Here is today's Workflow activity for {{tenant}}.",
    "newWorkflow": "New workflow",
    "recent": "Recent workflows",
    "metrics": {
      "running": "Running",
      "today": "Runs today",
      "successRate": "Success rate",
      "cost": "Cost today",
      "realtime": "Live",
      "budget": "62% of budget",
      "executions": "executions"
    }
  },
  "quota": {
    "execution_concurrency": {
      "label": "Workflow concurrency",
      "unit": "executions",
      "period": "concurrent capacity"
    },
    "node_concurrency": {
      "label": "Node concurrency",
      "unit": "nodes",
      "period": "concurrent capacity"
    },
    "sandbox_concurrency": {
      "label": "Sandbox concurrency",
      "unit": "sandboxes",
      "period": "concurrent capacity"
    },
    "agent_iterations": {
      "label": "Agent iteration quota",
      "unit": "iterations",
      "period": "per day"
    },
    "tokens": {
      "label": "Token quota",
      "unit": "tokens",
      "period": "per day"
    },
    "cost_micros": {
      "label": "Model cost quota",
      "unit": "currency micro-units",
      "period": "per day"
    },
    "artifact_bytes": {
      "label": "Artifact storage",
      "unit": "GB",
      "period": "storage capacity"
    },
    "cpu_millis": {
      "label": "Sandbox CPU capacity",
      "unit": "CPU cores",
      "period": "concurrent capacity"
    },
    "memory_bytes": {
      "label": "Sandbox memory capacity",
      "unit": "GB",
      "period": "concurrent capacity"
    },
    "pids": {
      "label": "Sandbox process capacity",
      "unit": "processes",
      "period": "concurrent capacity"
    },
    "disk_bytes": {
      "label": "Sandbox disk capacity",
      "unit": "GB",
      "period": "concurrent capacity"
    },
    "ttl_seconds": {
      "label": "Sandbox runtime capacity",
      "unit": "hours",
      "period": "concurrent capacity"
    }
  },
  "capability": {
    "plugin_nodejs": "Canvas plugins (Node.js)",
    "agent": "Agent nodes",
    "builtin": "Built-in nodes",
    "declarative_http": "HTTP request nodes",
    "mcp_tool": "MCP tools",
    "memory": "Memory read/write",
    "model": "Model calls",
    "rag": "Knowledge retrieval",
    "sandbox": "Sandbox execution",
    "skill": "Skill capabilities"
  },
  "capabilityStatus": {
    "ready": "Available",
    "unavailable": "Unavailable",
    "unknown": "Unknown"
  },
  "retentionDataType": {
    "artifact": "Artifact files",
    "trace": "Execution traces",
    "application_message": "Application messages",
    "evaluation_report": "Evaluation reports"
  },
  "retentionStatus": {
    "candidate": "Pending cleanup",
    "deleted": "Cleaned",
    "blocked": "Protected",
    "failed": "Failed",
    "pending": "Pending"
  },
  "retentionReasons": {
    "expired": "Retention period exceeded", "referenced": "Still referenced", "legal_hold": "Under retention hold", "deletion_failed": "Cleanup failed"
  }
} as const

export default translations
