import { Bar, BarChart, CartesianGrid, Cell, ResponsiveContainer, Tooltip, XAxis, YAxis } from 'recharts'

import { chartMargins, chartPalette, tickStyle } from './theme'

export function TopBarChart({ data, valueKey, labelKey = 'bucket', height = 220, colorFor, onSelect }: { data: Array<Record<string, unknown>>, valueKey: string, labelKey?: string, height?: number, colorFor?: (row: Record<string, unknown>) => string; onSelect?: (row: Record<string, unknown>) => void }) {
  return <div style={{ height }}>
    <ResponsiveContainer height="100%" width="100%">
      <BarChart data={data} layout="vertical" margin={chartMargins}>
        <CartesianGrid stroke={chartPalette.grid} strokeDasharray="3 3" horizontal={false} />
        <XAxis stroke={chartPalette.grid} tick={tickStyle} tickLine={false} type="number" />
        <YAxis dataKey={labelKey} stroke={chartPalette.grid} tick={tickStyle} tickLine={false} width={140} type="category" />
        <Tooltip contentStyle={{ background: chartPalette.surface, border: `1px solid ${chartPalette.grid}`, borderRadius: 8, fontSize: 11 }} />
        <Bar cursor={onSelect ? 'pointer' : undefined} onClick={(item) => { if (item.payload) onSelect?.(item.payload as Record<string, unknown>) }} dataKey={valueKey} fill={chartPalette.primary} radius={[0, 4, 4, 0]}>
          {data.map((row, index) => <Cell fill={colorFor ? colorFor(row) : chartPalette.primary} key={index} />)}
        </Bar>
      </BarChart>
    </ResponsiveContainer>
  </div>
}
