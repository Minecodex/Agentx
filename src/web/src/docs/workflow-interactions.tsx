import { useTranslation } from 'react-i18next'

import { CodeSnippet } from './applications/code-snippet'

export function WaitInputDocumentation() {
  const { t } = useTranslation()
  return <div className="space-y-3 text-xs leading-5" data-testid="await-input-documentation">
    <p className="text-muted-foreground">{t('interaction.hookHint')}</p>
    <details><summary className="cursor-pointer font-medium">{t('interaction.notificationExample')}</summary><div className="mt-2"><CodeSnippet language="json" code={JSON.stringify({ eventId: '<event-id>', type: 'workflow.input_required', invocationId: '<invocation-id>', executionId: '<execution-id>', nodeId: '<node-id>', waitId: '<wait-id>', resumeUrl: '/gateway/v1/invocations/<invocation-id>/waits/<wait-id>/input', expiresAt: null }, null, 2)} /></div></details>
    <details><summary className="cursor-pointer font-medium">{t('interaction.callbackExample')}</summary><div className="mt-2"><CodeSnippet language="bash" code={'curl -X POST "$AGENTX_BASE_URL$RESUME_URL" \\\n  -H "Authorization: Bearer $AGENTX_API_KEY" \\\n  -H "Idempotency-Key: reply-001" \\\n  -H "Content-Type: application/json" \\\n  -d \'{"content":"确认"}\''} /></div></details>
    <p>{t('interaction.apiHelp')}</p><p>{t('interaction.channelHelp')}</p><p>{t('interaction.presetHelp')}</p><p>{t('interaction.rawHelp')}</p>
  </div>
}
