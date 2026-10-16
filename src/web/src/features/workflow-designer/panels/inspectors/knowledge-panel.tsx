import { useTranslation } from 'react-i18next'
import { CommonPanelSections, OutputContractHint, PanelHint, PanelSection, ParameterControl, ResourceSelectorFields, type ActionPanelProps } from './panel-shell'

export function KnowledgePanel(panel: ActionPanelProps) {
  const { t } = useTranslation()
  return <div className="space-y-5" data-testid="knowledge-panel">
    <PanelSection title={t('studio.panels.knowledge.resource')}>
      <ResourceSelectorFields panel={panel} />
    </PanelSection>
    <PanelSection title={t('studio.panels.knowledge.retrieval')}>
      <div className="space-y-4"><ParameterControl name="query" panel={panel} /><ParameterControl name="topK" panel={panel} /></div>
    </PanelSection>
    <PanelHint>{t('studio.panels.knowledge.outputHint')}</PanelHint>
    <OutputContractHint manifest={panel.manifest} />
    <CommonPanelSections panel={panel} />
  </div>
}
