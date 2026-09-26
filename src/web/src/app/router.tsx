/* oxlint-disable react/only-export-components */
import { lazy, Suspense, type ReactNode } from 'react'
import { createBrowserRouter } from 'react-router-dom'

import { DashboardPage } from '../features/dashboard/dashboard-page'
import { NotFoundPage } from '../features/not-found/not-found-page'
import { EnterpriseLayout } from '../layouts/enterprise-workbench/enterprise-layout'
import { Protected, PublicRoute, RequireAnyPermission, RequirePermission } from './auth-guards'
import { SetupPage } from '../features/auth/setup-page'
import { LoginPage } from '../features/auth/login-page'
import { ChangePasswordPage } from '../features/auth/change-password-page'
import { ForbiddenPage } from '../features/auth/forbidden-page'

const ApplicationsPage = lazy(() => import('../features/applications/applications-page').then((module) => ({ default: module.ApplicationsPage })))
const ApplicationDetailPage = lazy(() => import('../features/applications/application-detail-page').then((module) => ({ default: module.ApplicationDetailPage })))
const ApprovalsPage = lazy(() => import('../features/approvals/approvals-page').then((module) => ({ default: module.ApprovalsPage })))
const ApprovalDetailPage = lazy(() => import('../features/approvals/approval-detail-page').then((module) => ({ default: module.ApprovalDetailPage })))
const ResourceGrantRequestDetailPage = lazy(() => import('../features/approvals/resource-grant-request-detail-page').then((module) => ({ default: module.ResourceGrantRequestDetailPage })))
const DatasetsPage = lazy(() => import('../features/datasets/datasets-page').then((module) => ({ default: module.DatasetsPage })))
const DatasetDetailPage = lazy(() => import('../features/datasets/dataset-detail-page').then((module) => ({ default: module.DatasetDetailPage })))
const EvaluationsPage = lazy(() => import('../features/evaluations/evaluations-page').then((module) => ({ default: module.EvaluationsPage })))
const EvaluationDetailPage = lazy(() => import('../features/evaluations/evaluation-detail-page').then((module) => ({ default: module.EvaluationDetailPage })))
const ExecutionsPage = lazy(() => import('../features/executions/executions-page').then((module) => ({ default: module.ExecutionsPage })))
const ExecutionDetailPage = lazy(() => import('../features/executions/execution-detail-page').then((module) => ({ default: module.ExecutionDetailPage })))
const NotificationsPage = lazy(() => import('../features/notifications/notifications-page').then((module) => ({ default: module.NotificationsPage })))
const RuntimeStatusPage = lazy(() => import('../features/runtime/runtime-status-page').then((module) => ({ default: module.RuntimeStatusPage })))
const AgentSessionsPage = lazy(() => import('../features/agent-sessions/agent-sessions-page').then((module) => ({ default: module.AgentSessionsPage })))
const KnowledgePage = lazy(() => import('../features/knowledge/knowledge-page').then((module) => ({ default: module.KnowledgePage })))
const KnowledgeDetailPage = lazy(() => import('../features/knowledge/knowledge-detail-page').then((module) => ({ default: module.KnowledgeDetailPage })))
const MemoryPage = lazy(() => import('../features/memory/memory-page').then((module) => ({ default: module.MemoryPage })))
const MemoryDetailPage = lazy(() => import('../features/memory/memory-detail-page').then((module) => ({ default: module.MemoryDetailPage })))
const ModelsPage = lazy(() => import('../features/models/models-page').then((module) => ({ default: module.ModelsPage })))
const ModelDetailPage = lazy(() => import('../features/models/model-detail-page').then((module) => ({ default: module.ModelDetailPage })))
const OrganizationPage = lazy(() => import('../features/organization/organization-page').then((module) => ({ default: module.OrganizationPage })))
const PlaygroundPage = lazy(() => import('../features/playground/playground-page').then((module) => ({ default: module.PlaygroundPage })))
const RolesPage = lazy(() => import('../features/roles/roles-page').then((module) => ({ default: module.RolesPage })))
const SkillsPage = lazy(() => import('../features/skills/skills-page').then((module) => ({ default: module.SkillsPage })))
const SkillDetailPage = lazy(() => import('../features/skills/skill-detail-page').then((module) => ({ default: module.SkillDetailPage })))
const McpServersPage = lazy(() => import('../features/mcp/mcp-servers-page').then((module) => ({ default: module.McpServersPage })))
const McpServerDetailPage = lazy(() => import('../features/mcp/mcp-server-detail-page').then((module) => ({ default: module.McpServerDetailPage })))
const CanvasPluginsPage = lazy(() => import('../features/canvas-plugins/canvas-plugins-page').then((module) => ({ default: module.CanvasPluginsPage })))
const CanvasPluginDetailPage = lazy(() => import('../features/canvas-plugins/canvas-plugin-detail-page').then((module) => ({ default: module.CanvasPluginDetailPage })))
const WorkflowsPage = lazy(() => import('../features/workflows/workflows-page').then((module) => ({ default: module.WorkflowsPage })))
const EnvironmentsPage = lazy(() => import('../features/workflows/environments-page').then((module) => ({ default: module.EnvironmentsPage })))
const WorkflowDetailPage = lazy(() => import('../features/workflows/workflow-detail-page').then((module) => ({ default: module.WorkflowDetailPage })))
const WorkflowCanvas = lazy(() => import('../features/workflow-designer/workflow-canvas').then((module) => ({ default: module.WorkflowCanvas })))
const CredentialsPage = lazy(() => import('../features/credentials/credentials-page').then((module) => ({ default: module.CredentialsPage })))
const CredentialDetailPage = lazy(() => import('../features/credentials/credential-detail-page').then((module) => ({ default: module.CredentialDetailPage })))
const ResourceGrantsPage = lazy(() => import('../features/resource-grants/resource-grants-page').then((module) => ({ default: module.ResourceGrantsPage })))
const SandboxProfilesPage = lazy(() => import('../features/sandbox-profiles/sandbox-profiles-page').then((module) => ({ default: module.SandboxProfilesPage })))
const SandboxProfileDetailPage = lazy(() => import('../features/sandbox-profiles/sandbox-profile-detail-page').then((module) => ({ default: module.SandboxProfileDetailPage })))
const InsightsPage = lazy(() => import('../features/insights/insights-page').then((m) => ({ default: m.InsightsPage })))

function deferred(element: ReactNode) {
  return <Suspense fallback={<div className="grid h-full min-h-72 place-items-center text-sm text-muted-foreground">Agentx…</div>}>{element}</Suspense>
}

export const router = createBrowserRouter([
  { path: 'setup', element: <PublicRoute kind="setup"><SetupPage /></PublicRoute> },
  { path: 'login', element: <PublicRoute kind="login"><LoginPage /></PublicRoute> },
  { path: 'change-password', element: <PublicRoute kind="change-password"><ChangePasswordPage /></PublicRoute> },
  { path: '403', element: <Protected><ForbiddenPage /></Protected> },
  { path: 'workflows/:workflowId/editor', element: <Protected>{deferred(<RequirePermission permission="workflow:edit"><WorkflowCanvas /></RequirePermission>)}</Protected> },
  {
    element: <Protected><EnterpriseLayout /></Protected>,
    children: [
      { index: true, element: <DashboardPage /> },
      { path: 'workflows', element: deferred(<RequirePermission permission="workflow:view"><WorkflowsPage /></RequirePermission>) },
      { path: 'environments', element: deferred(<RequirePermission permission="workflow:view"><EnvironmentsPage /></RequirePermission>) },
      { path: 'workflows/:workflowId', element: deferred(<RequirePermission permission="workflow:view"><WorkflowDetailPage /></RequirePermission>) },
      { path: 'applications', element: deferred(<RequirePermission permission="application:view"><ApplicationsPage /></RequirePermission>) },
      { path: 'applications/:id', element: deferred(<RequirePermission permission="application:view"><ApplicationDetailPage /></RequirePermission>) },
      { path: 'playground', element: deferred(<RequirePermission permission="application:invoke"><PlaygroundPage /></RequirePermission>) },
      { path: 'executions', element: deferred(<RequirePermission permission="execution:view"><ExecutionsPage /></RequirePermission>) },
      { path: 'executions/:id', element: deferred(<RequireAnyPermission permissions={['execution:view', 'application:invoke']}><ExecutionDetailPage /></RequireAnyPermission>) },
      { path: 'approvals', element: deferred(<RequirePermission permission="approval:view"><ApprovalsPage /></RequirePermission>) },
      { path: 'approvals/resource-grants/:id', element: deferred(<RequirePermission permission="workflow:view"><ResourceGrantRequestDetailPage /></RequirePermission>) },
      { path: 'approvals/:id', element: deferred(<RequirePermission permission="approval:view"><ApprovalDetailPage /></RequirePermission>) },
      { path: 'notifications', element: deferred(<RequirePermission permission="notification:view"><NotificationsPage /></RequirePermission>) },
      { path: 'datasets', element: deferred(<RequirePermission permission="dataset:view"><DatasetsPage /></RequirePermission>) },
      { path: 'datasets/:id', element: deferred(<RequirePermission permission="dataset:view"><DatasetDetailPage /></RequirePermission>) },
      { path: 'insights', element: deferred(<RequirePermission permission="execution:view"><InsightsPage /></RequirePermission>) },
      { path: 'evaluations', element: deferred(<RequirePermission permission="evaluation:view"><EvaluationsPage /></RequirePermission>) },
      { path: 'evaluations/:id', element: deferred(<RequirePermission permission="evaluation:view"><EvaluationDetailPage /></RequirePermission>) },
      { path: 'runtime', element: deferred(<RequirePermission permission="runtime:view"><RuntimeStatusPage /></RequirePermission>) },
      { path: 'agent-sessions', element: deferred(<RequirePermission permission="execution:view"><AgentSessionsPage /></RequirePermission>) },
      { path: 'credentials', element: deferred(<RequirePermission permission="credential:view"><CredentialsPage /></RequirePermission>) },
      { path: 'credentials/:id', element: deferred(<RequirePermission permission="credential:view"><CredentialDetailPage /></RequirePermission>) },
      { path: 'models', element: deferred(<RequirePermission permission="model:view"><ModelsPage /></RequirePermission>) },
      { path: 'models/:id', element: deferred(<RequirePermission permission="model:view"><ModelDetailPage /></RequirePermission>) },
      { path: 'mcp', element: deferred(<RequirePermission permission="mcp:view"><McpServersPage /></RequirePermission>) },
      { path: 'mcp/:id', element: deferred(<RequirePermission permission="mcp:view"><McpServerDetailPage /></RequirePermission>) },
      { path: 'canvas-plugins', element: deferred(<RequirePermission permission="canvas_plugin:view"><CanvasPluginsPage /></RequirePermission>) },
      { path: 'canvas-plugins/:id', element: deferred(<RequirePermission permission="canvas_plugin:view"><CanvasPluginDetailPage /></RequirePermission>) },
      { path: 'skills', element: deferred(<RequirePermission permission="skill:view"><SkillsPage /></RequirePermission>) },
      { path: 'skills/:id', element: deferred(<RequirePermission permission="skill:view"><SkillDetailPage /></RequirePermission>) },
      { path: 'knowledge', element: deferred(<RequirePermission permission="knowledge:view"><KnowledgePage /></RequirePermission>) },
      { path: 'knowledge/:id', element: deferred(<RequirePermission permission="knowledge:view"><KnowledgeDetailPage /></RequirePermission>) },
      { path: 'memory', element: deferred(<RequirePermission permission="memory:view"><MemoryPage /></RequirePermission>) },
      { path: 'memory/:id', element: deferred(<RequirePermission permission="memory:view"><MemoryDetailPage /></RequirePermission>) },
      { path: 'sandbox-profiles', element: deferred(<RequirePermission permission="sandbox:view"><SandboxProfilesPage /></RequirePermission>) },
      { path: 'sandbox-profiles/:id', element: deferred(<RequirePermission permission="sandbox:view"><SandboxProfileDetailPage /></RequirePermission>) },
      { path: 'resource-grants', element: deferred(<RequirePermission permission="resource:grant"><ResourceGrantsPage /></RequirePermission>) },
      { path: 'organization', element: deferred(<RequirePermission permission="user:view"><OrganizationPage /></RequirePermission>) },
      { path: 'roles', element: deferred(<RequirePermission permission="role:view"><RolesPage /></RequirePermission>) },
      { path: '*', element: <NotFoundPage /> },
    ],
  },
])
