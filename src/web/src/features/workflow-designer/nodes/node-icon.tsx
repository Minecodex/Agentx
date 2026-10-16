import { ArrowDownAZ, BadgeCheck, Binary, BookOpen, Bot, Box, Braces, BrainCircuit, CalendarClock, Circle, Clock3, Code2, CopyMinus, Database, Filter, GitCompare, GitMerge, Hash, ListEnd, ListPlus, MemoryStick, MousePointerClick, OctagonX, RadioTower, Repeat2, Replace, Rows3, ShieldCheck, Sigma, Sparkles, Split, Wrench, type LucideIcon } from 'lucide-react'

const icons: Record<string, LucideIcon> = {
  'arrow-down-a-z': ArrowDownAZ, 'badge-check': BadgeCheck, binary: Binary, 'book-open': BookOpen, bot: Bot, box: Box, braces: Braces, 'brain-circuit': BrainCircuit, 'calendar-clock': CalendarClock, circle: Circle, 'clock-3': Clock3, 'code-2': Code2, 'copy-minus': CopyMinus, database: Database, filter: Filter, 'git-compare': GitCompare, 'git-merge': GitMerge, hash: Hash, 'list-end': ListEnd, 'list-plus': ListPlus, 'memory-stick': MemoryStick, 'mouse-pointer-click': MousePointerClick, 'octagon-x': OctagonX, 'radio-tower': RadioTower, 'repeat-2': Repeat2, replace: Replace, 'rows-3': Rows3, 'shield-check': ShieldCheck, sigma: Sigma, sparkles: Sparkles, split: Split, wrench: Wrench,
}

export function NodeIcon({ iconKey, className }: { iconKey: string; className?: string }) {
  const Icon = icons[iconKey] ?? Box
  return <Icon className={className} />
}
