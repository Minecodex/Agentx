const translations = {
  "assignee": "Assignee",
  "review": "Review",
  "claim": "Claim",
  "approve": "Approve",
  "reject": "Reject",
  "release": "Release",
  "approvalUpdated": "Approval updated",
  "resumeStatus": "Resume status",
  "decision": "Decision",
  "reason": "Reason (optional)",
  "request": "Approval request",
  "actionHistory": "Action history",
  "trace": "View trace",
  "title": "Approvals",
  "description": "Process tasks created by Workflow approval nodes.",
  "search": "Search approvals",
  "workflow": "Workflow",
  "deadline": "Deadline",
  "createdAt": "Created",
  "resumeStatuses": {
    "not_requested": "Not requested", "pending": "Pending", "succeeded": "Resumed", "blocked_runtime": "Blocked by Runtime", "failed": "Resume failed"
  },
  "statuses": {
    "pending": "Pending", "claimed": "Claimed", "decided": "Decided", "cancelled": "Cancelled", "timed_out": "Timed out"
  },
  "actionTypes": {
    "claim": "Claim", "release": "Release", "reassign": "Reassign", "decide": "Decide", "cancel": "Cancel", "timeout": "Timeout"
  },
  "auditActions": {
    "stale": "Request invalidated",
    "created": "Request created", "submitted": "Request submitted", "approved": "Approved", "rejected": "Rejected", "cancelled": "Cancelled", "review_approved": "Department review approved", "review_rejected": "Department review rejected"
  }
  ,"runtimeTab": "Runtime approvals"
  ,"resourceGrantTab": "Resource access"
  ,"resourceGrantDescription": "Review design-time resource access requests and department co-signs."
  ,"resourceGrantSearch": "Search resources, workflows, or requesters"
  ,"requester": "Requester"
  ,"reviewProgress": "Review progress"
  ,"controlledResource": "Controlled resource"
  ,"resourceGrantPackage": "Authorization package"
  ,"resourceGrantNoMessage": "The requester needs this resource in the workflow."
  ,"controlledDependency": "Controlled dependency"
  ,"authorized": "Authorized"
  ,"resourceGrantReviews": "Department reviews"
  ,"awaitingReviewer": "Awaiting reviewer"
  ,"auditHistory": "Audit history"
  ,"systemActor": "System"
  ,"resourceGrantUpdated": "Resource access request updated"
  ,"confirmApprove": "Approve this resource authorization request?"
  ,"confirmResourceReject": "Reject this resource authorization request?"
  ,"resourceGrantStatuses": {
    "pending": "Pending",
    "approved": "Approved",
    "rejected": "Rejected",
    "cancelled": "Cancelled",
    "stale": "Stale"
  }
} as const

export default translations
