// Shared chart theme (plan7 P7-C): recharts colors mapped to the semantic
// --ui-* tokens so light/dark themes stay consistent across every chart.
export const chartPalette = {
  primary: 'var(--ui-primary, #635bff)',
  success: 'var(--ui-success, #0f9f6e)',
  warning: 'var(--ui-warning, #d97706)',
  danger: 'var(--ui-danger, #dc4c64)',
  muted: 'var(--ui-muted, #8891a5)',
  axis: 'var(--ui-muted, #8891a5)',
  grid: 'var(--ui-border, #e4e7ec)',
  surface: 'var(--ui-canvas, #f7f8fa)',
} as const

export const chartMargins = { top: 8, right: 12, bottom: 4, left: 4 } as const
export const tickStyle = { fontSize: 10, fill: chartPalette.axis } as const
