import { AgentPanel } from '../inspectors/agent-panel'
import { AwaitInputPanel } from '../inspectors/await-input-panel'
import { ReplyMessagePanel } from '../inspectors/reply-message-panel'
import { ApprovalPanel } from '../inspectors/approval-panel'
import { CodePanel } from '../inspectors/code-panel'
import { IfPanel } from '../inspectors/if-panel'
import { LoopPanel } from '../inspectors/loop-panel'
import { MergePanel } from '../inspectors/merge-panel'
import { KnowledgePanel } from '../inspectors/knowledge-panel'
import { ModelPanel } from '../inspectors/model-panel'
import { SubworkflowPanel } from '../inspectors/subworkflow-panel'
import type { BuiltinNodeUiPackage } from './types'

export const coreNodeUiPackage: BuiltinNodeUiPackage = {
  packageId: 'agentx/core',
  packageVersion: '1.0.0',
  panels: {
    agent: AgentPanel,
    approval: ApprovalPanel,
    await_input: AwaitInputPanel,
    reply_message: ReplyMessagePanel,
    code: CodePanel,
    if: IfPanel,
    loop_over_items: LoopPanel,
    merge: MergePanel,
    model: ModelPanel,
    knowledge_retrieval: KnowledgePanel,
    sub_workflow: SubworkflowPanel,
  },
}
