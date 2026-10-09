const translations = {
  "assignee": "处理人",
  "review": "处理",
  "claim": "领取",
  "approve": "通过",
  "reject": "拒绝",
  "release": "释放",
  "approvalUpdated": "审批状态已更新",
  "resumeStatus": "恢复状态",
  "decision": "决策结果",
  "reason": "理由（可选）",
  "request": "审批请求",
  "actionHistory": "动作记录",
  "trace": "查看 Trace",
  "title": "待审批",
  "description": "处理工作流审批节点产生的任务。",
  "search": "搜索审批事项",
  "workflow": "工作流",
  "deadline": "截止时间",
  "createdAt": "发起时间",
  "resumeStatuses": {
    "not_requested": "未请求恢复", "pending": "等待恢复", "succeeded": "已恢复", "blocked_runtime": "Runtime 阻塞", "failed": "恢复失败"
  },
  "statuses": {
    "pending": "待领取", "claimed": "处理中", "decided": "已决策", "cancelled": "已取消", "timed_out": "已超时"
  },
  "actionTypes": {
    "claim": "领取", "release": "释放", "reassign": "转交", "decide": "决策", "cancel": "取消", "timeout": "超时"
  },
  "auditActions": {
    "stale": "申请已失效",
    "created": "创建申请", "submitted": "提交申请", "approved": "通过", "rejected": "拒绝", "cancelled": "取消", "review_approved": "部门会签通过", "review_rejected": "部门会签拒绝"
  }
  ,"runtimeTab": "运行审批"
  ,"resourceGrantTab": "资源授权"
  ,"resourceGrantDescription": "处理工作流设计期的资源授权申请与部门会签。"
  ,"resourceGrantSearch": "搜索资源、工作流或申请人"
  ,"requester": "申请人"
  ,"reviewProgress": "会签进度"
  ,"controlledResource": "受控资源"
  ,"resourceGrantPackage": "授权包"
  ,"resourceGrantNoMessage": "申请人需要在工作流中使用此资源。"
  ,"controlledDependency": "受控依赖"
  ,"authorized": "已授权"
  ,"resourceGrantReviews": "部门会签"
  ,"awaitingReviewer": "等待审批人处理"
  ,"auditHistory": "审计历史"
  ,"systemActor": "系统"
  ,"resourceGrantUpdated": "资源授权申请已更新"
  ,"confirmApprove": "确认通过该资源授权申请？"
  ,"confirmResourceReject": "确认拒绝该资源授权申请？"
  ,"resourceGrantStatuses": {
    "pending": "审批中",
    "approved": "已通过",
    "rejected": "已拒绝",
    "cancelled": "已取消",
    "stale": "已失效"
  }
} as const

export default translations
