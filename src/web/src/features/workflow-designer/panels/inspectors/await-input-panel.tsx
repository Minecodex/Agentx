import { useTranslation } from 'react-i18next'

import { WaitInputDocumentation } from '../../../../docs/workflow-interactions'
import { CommonPanelSections, PanelHint, PanelSection, ParameterControl, OutputContractHint, type ActionPanelProps } from './panel-shell'

export function AwaitInputPanel(panel: ActionPanelProps) {
  const { t } = useTranslation()
  return <div className="space-y-5" data-testid="await-input-panel">
    <PanelSection title={t('interaction.waitSettings')}><ParameterControl name="timeoutMs" panel={panel} /><div className="mt-2"><PanelHint>{t('interaction.timeoutHint')}</PanelHint></div></PanelSection>
    <PanelSection title={t('interaction.outputs')}><OutputContractHint manifest={panel.manifest} /></PanelSection>
    <CommonPanelSections panel={panel} />
    <PanelSection title={t('interaction.exits')} testId="await-input-exits"><div className="space-y-2 rounded-md border border-border p-3 text-xs">{[t('interaction.received'), t('interaction.timedOut'), t('interaction.error')].map((key, index) => <div key={key} className="flex gap-3"><code className="w-20 shrink-0 text-muted-foreground">{['main', 'timed_out', 'error'][index]}</code><span>{key}</span></div>)}</div></PanelSection>
    <PanelSection title={t('interaction.hook')} testId="await-input-hook"><ParameterControl name="hookUrl" panel={panel} /><div className="mt-3"><WaitInputDocumentation /></div></PanelSection>
  </div>
}
