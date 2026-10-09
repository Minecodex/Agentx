import { CartesianGrid, Line, LineChart, ResponsiveContainer, Tooltip, XAxis, YAxis } from 'recharts'

import { chartMargins, chartPalette, tickStyle } from './theme'

export type TrendSeries = { key: string, label: string, color?: string }

export function TrendChart({ data, series, valueSuffix = '', height = 220 }: { data: Array<Record<string, unknown>>, series: TrendSeries[], valueSuffix?: string, height?: number }) {
  return <div style={{ height }}>
    <ResponsiveContainer height="100%" width="100%">
      <LineChart data={data} margin={chartMargins}>
        <CartesianGrid stroke={chartPalette.grid} strokeDasharray="3 3" vertical={false} />
        <XAxis dataKey="bucket" stroke={chartPalette.grid} tick={tickStyle} tickLine={false} />
        <YAxis stroke={chartPalette.grid} tick={tickStyle} tickLine={false} width={44} />
        <Tooltip contentStyle={{ background: chartPalette.surface, border: `1px solid ${chartPalette.grid}`, borderRadius: 8, fontSize: 11 }} formatter={(value) => [`${String(value)}${valueSuffix}`]} />
        {series.map((item, index) => <Line dataKey={item.key} dot={false} key={item.key} name={item.label} stroke={item.color ?? [chartPalette.primary, chartPalette.success, chartPalette.warning, chartPalette.danger][index % 4]} strokeWidth={2} type="monotone" />)}
      </LineChart>
    </ResponsiveContainer>
  </div>
}
