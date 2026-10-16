import { useTranslation } from "react-i18next";

import { Select } from "../../../../shared/ui/select";
import type { ReferenceCatalog, ReferenceEntry } from "../../model/types";
import {
  CommonPanelSections,
  ModeCardsGroup,
  PanelHint,
  PanelSection,
  ParameterControl,
  type ActionPanelProps,
} from "./panel-shell";

const MODE_PARAMETER = "mode";

export function MergePanel(panel: ActionPanelProps) {
  const { t } = useTranslation();
  const modeSchema = panel.manifest.parameterSchema.properties?.[MODE_PARAMETER];
  const mode = String(panel.data.parameters[MODE_PARAMETER] ?? modeSchema?.default ?? "append");
  const fields = mergeFieldOptions(panel.referenceCatalog);
  const update = (patch: Record<string, unknown>) => panel.onChange({ parameters: patch });
  return (
    <div className="space-y-5" data-testid="merge-panel">
      <PanelSection title={t("studio.panels.merge.mode")} testId="merge-mode-section">
        <ModeCardsGroup onChange={(value) => update({ [MODE_PARAMETER]: value })} options={(modeSchema?.enum ?? []).map(String).map((value) => ({ value, label: panel.localized?.parameterEnumLabel(MODE_PARAMETER, value) ?? value, description: t(`studio.panels.merge.modeDescriptions.${value}`) }))} value={mode} />
        <div className="mt-3 rounded-md bg-muted/35 p-2 font-mono text-[10px] leading-5 text-muted-foreground">{t(`studio.panels.merge.examples.${mode}`)}</div>
        {mode === "combine_by_key" && <div className="mt-4 grid grid-cols-2 gap-3" data-testid="merge-join-fields">
          <label className="text-xs text-muted-foreground" data-testid="merge-left-field">{t("studio.panels.merge.leftField")}<Select className="mt-1 w-full" onValueChange={(leftField) => update({ leftField })} options={fields} value={String(panel.data.parameters.leftField ?? "id")} /></label>
          <label className="text-xs text-muted-foreground" data-testid="merge-right-field">{t("studio.panels.merge.rightField")}<Select className="mt-1 w-full" onValueChange={(rightField) => update({ rightField })} options={fields} value={String(panel.data.parameters.rightField ?? "id")} /></label>
          <div className="col-span-2"><ParameterControl name="joinType" panel={panel} /></div>
        </div>}
        {mode && mode !== "append" && <div className="mt-4"><ParameterControl name="conflictStrategy" panel={panel} /></div>}
      </PanelSection>
      <PanelSection title={t("studio.panels.merge.inputs")} testId="merge-inputs-section">
        <PanelHint>{mode === "append" ? t("studio.panels.merge.appendInputsHint") : t("studio.panels.merge.pairInputsHint")}</PanelHint>
      </PanelSection>
      <CommonPanelSections panel={panel} />
    </div>
  );
}

function mergeFieldOptions(catalog?: ReferenceCatalog) {
  const names = new Set<string>(["id"]);
  const visit = (entries: ReferenceEntry[]) => {
    for (const entry of entries) {
      if (entry.selector?.path?.length) names.add(entry.selector.path.map(String).join("."));
      visit(entry.children);
    }
  };
  for (const entries of Object.values(catalog ?? {})) visit(entries ?? []);
  return [...names].sort().map((value) => ({ value, label: value }));
}
