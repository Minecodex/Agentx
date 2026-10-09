import type { ColumnDef } from '@tanstack/react-table'
import { Plus } from 'lucide-react'
import type { ReactNode } from 'react'
import { useTranslation } from 'react-i18next'

import { Card } from '../ui/card'
import { ComingSoonAction } from './coming-soon-action'
import { DataTable } from './data-table'
import { EmptyState } from './empty-state'
import { PageContainer } from './page-container'
import { PageHeader } from './page-header'

type ListPageProps<T> = {
  title: string
  description: string
  action?: ReactNode
  actionLabel?: string
  searchPlaceholder: string
  columns: Array<ColumnDef<T>>
  data: T[]
  getSearchText: (row: T) => string
  getStatus?: (row: T) => string
  statusOptions?: Array<{ value: string; label: string }>
  getCategory?: (row: T) => string
  categoryOptions?: Array<{ value: string; label: string }>
  categoryLabel?: string
  tableHeader?: ReactNode
  loading?: boolean
  error?: Error | null
}

export function ListPage<T>({ title, description, action, actionLabel, loading, error, ...tableProps }: ListPageProps<T>) {
  const { t } = useTranslation()
  return (
    <PageContainer>
      <PageHeader action={action ?? (actionLabel ? <ComingSoonAction><Plus className="size-4" />{actionLabel}</ComingSoonAction> : undefined)} description={description} title={title} />
      {error || loading ? <Card className="p-8" role={error ? 'alert' : 'status'}><EmptyState title={t(error ? 'common.loadFailed' : 'common.loading')} description={error?.message} /></Card> : <DataTable {...tableProps} />}
    </PageContainer>
  )
}
