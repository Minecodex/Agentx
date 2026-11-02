import { useMutation } from '@tanstack/react-query'
import { useTranslation } from 'react-i18next'
import { apiRequest } from '../../../shared/api/client'
import type { components } from '../../../shared/api/generated'
import { Button } from '../../../shared/ui/button'
import { Card } from '../../../shared/ui/card'

type Diagnostic=components['schemas']['ApplicationMcpDiagnostic']
export function McpDiagnostics({applicationId,publicationId}:{applicationId:string;publicationId:string}){
  const {t}=useTranslation()
  const check=useMutation({mutationFn:()=>apiRequest<Diagnostic>(`/applications/${applicationId}/mcp/diagnostics?publicationId=${publicationId}`)})
  return <Card className="space-y-4 p-5" data-testid="application-mcp-diagnostics">
    <div className="flex items-center justify-between gap-4"><div><h2 className="text-sm font-semibold">{t('applicationMcp.diagnose')}</h2><p className="mt-1 text-xs text-muted-foreground">{t('applicationMcp.diagnosticScope')}</p></div><Button size="sm" variant="secondary" disabled={check.isPending} onClick={()=>check.mutate()}>{t(check.isPending?'common.loading':'applicationMcp.diagnose')}</Button></div>
    {check.error&&<p className="text-sm text-danger" role="alert">{check.error.message}</p>}
    {check.data&&<div className="space-y-3" aria-live="polite"><p className={`text-sm ${check.data.passed?'text-success':'text-danger'}`}>{t(check.data.passed?'applicationMcp.diagnosticPassed':'applicationMcp.diagnosticFailed')}</p><ul className="grid grid-cols-2 gap-2 text-xs">{check.data.checks.map(item=><li key={item.name} className="flex items-center justify-between rounded-lg border border-border p-3"><span>{t(`applicationMcp.check_${item.name}`)}</span><span className={item.passed?'text-success':'text-danger'}>{t(item.passed?'applicationMcp.checkPassed':'applicationMcp.checkFailed')}</span></li>)}</ul></div>}
  </Card>
}
