import { LogIn, Pencil, Plus, Trash2, X } from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { InspectorShell } from "../inspector-shell";
import { ReadOnlyForm } from "../../forms/read-only-form";

import { Button } from "../../../../shared/ui/button";
import { Input } from "../../../../shared/ui/input";
import { Select } from "../../../../shared/ui/select";
import { groupColor } from "../../nodes/node-appearance";
import type { WorkflowStart } from "../../model/types";
import {
  asObject,
  asInputProperties,
  INPUT_TYPES,
  inputKind,
  internalNameError,
  InputFieldDialog,
  ContextForm,
  SectionTitle,
  schemaRequired,
  schemaForInputKind,
  type InputProperty,
  type ValueUpdater,
} from "../interface-fields";

const SYSTEM_VARIABLES = [
  { path: "execution.id", labelKey: "studio.executionReferences.executionId" },
  { path: "execution.startedAt", labelKey: "studio.executionReferences.startedAt" },
  { path: "execution.workflow.id", labelKey: "studio.executionReferences.workflowId" },
  { path: "execution.workflow.name", labelKey: "studio.executionReferences.workflowName" },
  { path: "execution.initiator.user.name", labelKey: "studio.executionReferences.userName" },
];

export function StartPanel({
  start,
  onStartChange,
  onClose,
  readOnly = false,
}: {
  start: WorkflowStart;
  onStartChange: (value: ValueUpdater<WorkflowStart>) => void;
  onClose: () => void;
  readOnly?: boolean;
}) {
  const { t } = useTranslation();
  return (
    <InspectorShell testId="start-panel">
      <header className="flex h-16 items-center gap-3 border-b border-border px-4">
        <span className="grid size-9 place-items-center rounded-md" style={{ color: groupColor('start'), backgroundColor: `color-mix(in srgb, ${groupColor('start')} 12%, transparent)` }}><LogIn className="size-5" /></span>
        <div className="min-w-0 flex-1"><strong className="block text-sm">{t("studio.interface.start")}</strong><span className="text-[10px] text-muted-foreground">{t("studio.interface.startDescription")}</span></div>
        <Button aria-label={t("common.close")} onClick={onClose} size="icon" variant="ghost"><X className="size-4" /></Button>
      </header>
      <ReadOnlyForm readOnly={readOnly} className="min-h-0 min-w-0 flex-1 space-y-6 overflow-y-auto p-4"><StartForm start={start} onChange={onStartChange} /></ReadOnlyForm>
    </InspectorShell>
  );
}

function StartForm({ start, onChange }: { start: WorkflowStart; onChange: (value: ValueUpdater<WorkflowStart>) => void }) {
  const { t } = useTranslation();
  const schema = asObject(start.inputs);
  const properties = asInputProperties(schema.properties);
  const required = schemaRequired(schema);
  const updateSchema = (update: (schema: Record<string, any>) => Record<string, unknown>) => onChange((current) => {
    const currentSchema = asObject(current.inputs);
    return { ...current, inputs: { type: "object", additionalProperties: false, ...currentSchema, ...update(currentSchema) } };
  });
  const [dialogOpen, setDialogOpen] = useState(false);
  const [editing, setEditing] = useState<{ name: string; property: InputProperty; required: boolean } | null>(null);
  const saveFrom = (previousName: string | undefined, name: string, property: InputProperty, isRequired: boolean) => updateSchema((currentSchema) => {
    const currentProperties = asInputProperties(currentSchema.properties);
    const nextProperties = { ...currentProperties };
    if (previousName && previousName !== name) delete nextProperties[previousName];
    nextProperties[name] = property;
    const nextRequired = schemaRequired(currentSchema).filter((item) => item !== previousName && item !== name);
    if (isRequired) nextRequired.push(name);
    return { properties: nextProperties, required: nextRequired };
  });
  const save = (name: string, property: InputProperty, isRequired: boolean) => saveFrom(editing?.name, name, property, isRequired);
  const remove = (name: string) => updateSchema((currentSchema) => { const next = { ...asInputProperties(currentSchema.properties) }; delete next[name]; return { properties: next, required: schemaRequired(currentSchema).filter((item) => item !== name) }; });
  return <>
    <section>
      <SectionTitle action={<Button onClick={() => { setEditing(null); setDialogOpen(true); }} size="sm" variant="secondary"><Plus className="size-3.5" />{t("studio.addField")}</Button>} description={t("studio.interface.inputsDescription")} title={t("studio.interface.inputs")} />
      <div className="space-y-2">{Object.entries(properties).map(([name, property]) => <StartInputRow
        key={name}
        name={name}
        property={property}
        required={required.includes(name)}
        names={Object.keys(properties)}
        onBasicChange={(nextName, nextType, nextRequired) => saveFrom(name, nextName, schemaForInputKind(nextType, property), nextRequired)}
        onDelete={() => remove(name)}
        onEdit={() => { setEditing({ name, property, required: required.includes(name) }); setDialogOpen(true); }}
      />)}</div>
    </section>
    <InputFieldDialog editing={editing} onOpenChange={setDialogOpen} onSave={(name, property, isRequired) => { save(name, property, isRequired); setDialogOpen(false); }} open={dialogOpen} properties={properties} />
    <ContextForm contexts={start.contexts} onChange={(update) => onChange((current) => ({ ...current, contexts: resolveUpdate(current.contexts, update) }))} />
    <section>
      <SectionTitle description={t("studio.interface.systemVariablesDescription")} title={t("studio.interface.systemVariables")} />
      <div className="space-y-1.5 rounded-md border border-border bg-muted/30 px-3 py-2.5">
        {SYSTEM_VARIABLES.map((variable) => (
          <div className="flex items-baseline gap-2 text-[10px]" key={variable.path}>
            <code className="shrink-0 font-mono text-muted-foreground">{variable.path}</code>
            <span className="min-w-0 flex-1 truncate text-muted-foreground/80">{t(variable.labelKey)}</span>
          </div>
        ))}
      </div>
    </section>
  </>;
}

function StartInputRow({
  name,
  property,
  required,
  names,
  onBasicChange,
  onEdit,
  onDelete,
}: {
  name: string;
  property: InputProperty;
  required: boolean;
  names: string[];
  onBasicChange: (name: string, type: string, required: boolean) => void;
  onEdit: () => void;
  onDelete: () => void;
}) {
  const { t } = useTranslation();
  const [draftName, setDraftName] = useState(name);
  useEffect(() => setDraftName(name), [name]);
  const error = internalNameError(draftName, Object.fromEntries(names.map((value) => [value, true])), name, t);
  const commitName = () => {
    if (error) return setDraftName(name);
    onBasicChange(draftName.trim(), inputKind(property), required);
  };
  return (
    <div className="rounded-md border border-border p-2" data-testid={`start-input-${name}`}>
      <div className="grid grid-cols-[minmax(0,1fr)_100px_auto_28px_28px] items-center gap-2">
        <Input aria-label={t("studio.interface.fieldName")} onBlur={commitName} onChange={(event) => setDraftName(event.target.value)} value={draftName} />
        <Select aria-label={t("studio.interface.fieldType")} onValueChange={(type) => onBasicChange(name, type, required)} options={INPUT_TYPES.map((type) => ({ value: type, label: t(`studio.schemaTypes.${type}`, type) }))} value={inputKind(property)} />
        <label className="flex items-center gap-1 text-[10px] text-muted-foreground"><input checked={required} className="size-4 accent-primary" onChange={(event) => onBasicChange(name, inputKind(property), event.target.checked)} type="checkbox" />{t("studio.interface.required")}</label>
        <Button aria-label={t("studio.editField")} onClick={onEdit} size="icon" variant="ghost"><Pencil className="size-3.5" /></Button>
        <Button aria-label={t("studio.removeField", { key: name })} onClick={onDelete} size="icon" variant="ghost"><Trash2 className="size-3.5" /></Button>
      </div>
      {property.description && <p className="mt-1 truncate text-[10px] text-muted-foreground">{property.description}</p>}
    </div>
  );
}

function resolveUpdate<T>(current: T, update: ValueUpdater<T>) { return typeof update === "function" ? (update as (value: T) => T)(current) : update; }
