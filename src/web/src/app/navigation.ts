import {
  AppWindow,
  Blocks,
  BrainCircuit,
  CircleGauge,
  History,
  Database,
  FlaskConical,
  KeyRound,
  Library,
  LockKeyhole,
  MemoryStick,
  PackageOpen,
  Play,
  Shield,
  ShieldCheck,
  Sparkles,
  UsersRound,
  ServerCog,
  Box,
  Bell,
  RadioTower,
  Activity,
} from 'lucide-react'

import type { NavigationGroup } from '../shared/types/app'

export const navigationGroups: NavigationGroup[] = [
  {
    labelKey: 'navigation.groups.workspace',
    items: [
      { labelKey: 'navigation.dashboard', path: '/', icon: CircleGauge, keywords: ['overview', 'home'] },
      { labelKey: 'navigation.workflows', path: '/workflows', icon: Blocks, keywords: ['agent', 'flow'], requiredPermission: 'workflow:view' },
      { labelKey: 'navigation.applications', path: '/applications', icon: AppWindow, keywords: ['deployment', 'api'], requiredPermission: 'application:view' },
      { labelKey: 'navigation.playground', path: '/playground', icon: Play, keywords: ['chat', 'test'], requiredPermission: 'application:invoke' },
    ],
  },
  {
    labelKey: 'navigation.groups.runtime',
    items: [
      { labelKey: 'navigation.executions', path: '/executions', icon: CircleGauge, keywords: ['trace', 'run'], requiredPermission: 'execution:view' },
      { labelKey: 'navigation.approvals', path: '/approvals', icon: ShieldCheck, keywords: ['review', 'todo'], requiredPermission: 'approval:view' },
      { labelKey: 'navigation.notifications', path: '/notifications', icon: Bell, keywords: ['inbox', 'message'], requiredPermission: 'notification:view' },
      { labelKey: 'navigation.datasets', path: '/datasets', icon: Database, keywords: ['cases', 'test'], requiredPermission: 'dataset:view' },
      { labelKey: 'navigation.insights', path: '/insights', icon: Activity, keywords: ['charts', 'cost'], requiredPermission: 'execution:view' },
      { labelKey: 'navigation.evaluations', path: '/evaluations', icon: FlaskConical, keywords: ['report', 'quality'], requiredPermission: 'evaluation:view' },
      { labelKey: 'navigation.runtime', path: '/runtime', icon: RadioTower, keywords: ['worker', 'queue'], requiredPermission: 'runtime:view' },
      { labelKey: 'navigation.agentSessions', path: '/agent-sessions', icon: History, keywords: ['agent', 'session', 'compaction', 'memory'], requiredPermission: 'execution:view' },
    ],
  },
  {
    labelKey: 'navigation.groups.resources',
    items: [
      { labelKey: 'navigation.credentials', path: '/credentials', icon: LockKeyhole, keywords: ['secret', 'key'], requiredPermission: 'credential:view' },
      { labelKey: 'navigation.models', path: '/models', icon: BrainCircuit, keywords: ['llm', 'provider'], requiredPermission: 'model:view' },
      { labelKey: 'navigation.mcp', path: '/mcp', icon: ServerCog, keywords: ['connector', 'server', 'tool'], requiredPermission: 'mcp:view' },
      { labelKey: 'navigation.canvasPlugins', path: '/canvas-plugins', icon: PackageOpen, keywords: ['plugin', 'node', 'canvas'], requiredPermission: 'canvas_plugin:view' },
      { labelKey: 'navigation.skills', path: '/skills', icon: Sparkles, keywords: ['skill', 'agent', 'capability'], requiredPermission: 'skill:view' },
      { labelKey: 'navigation.knowledge', path: '/knowledge', icon: Library, keywords: ['rag', 'lightrag'], requiredPermission: 'knowledge:view' },
      { labelKey: 'navigation.memory', path: '/memory', icon: MemoryStick, keywords: ['mem0'], requiredPermission: 'memory:view' },
      { labelKey: 'navigation.sandboxProfiles', path: '/sandbox-profiles', icon: Box, keywords: ['sandbox', 'profile', 'runner'], requiredPermission: 'sandbox:view' },
      { labelKey: 'navigation.resourceGrants', path: '/resource-grants', icon: Shield, keywords: ['grant', 'permission', 'resource'], requiredPermission: 'resource:grant' },
    ],
  },
  {
    labelKey: 'navigation.groups.organization',
    items: [
      { labelKey: 'navigation.organization', path: '/organization', icon: UsersRound, keywords: ['department', 'user'], requiredPermission: 'user:view' },
      { labelKey: 'navigation.roles', path: '/roles', icon: KeyRound, keywords: ['rbac', 'permission'], requiredPermission: 'role:view' },
    ],
  },
]

export const navigationItems = navigationGroups.flatMap((group) => group.items)
