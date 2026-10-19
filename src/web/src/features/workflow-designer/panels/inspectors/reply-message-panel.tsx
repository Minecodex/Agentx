import { useTranslation } from 'react-i18next'

import { CommonPanelSections, PanelHint, PanelSection, ParameterControl, OutputContractHint, type ActionPanelProps } from './panel-shell'

export function ReplyMessagePanel(panel: ActionPanelProps) {
  const { t } = useTranslation()
  return <div className="space-y-5" data-testid="reply-message-panel">
    <PanelSection title={t('interaction.replyContent')}><ParameterControl name="content" panel={panel} /></PanelSection>
    <PanelHint>{t('interaction.replyHint')}</PanelHint>
    <PanelSection title={t('interaction.outputs')}><OutputContractHint manifest={panel.manifest} /></PanelSection>
    <CommonPanelSections panel={panel} />
  </div>
}
