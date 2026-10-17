import { LogOut, Pencil, Plus, Trash2, X } from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { InspectorShell } from "../inspector-shell";
import { ReadOnlyForm } from "../../forms/read-only-form";

import { Button } from "../../../../shared/ui/button";
import { Dialog, DialogContent } from "../../../../shared/ui/dialog";
import { Input } from "../../../../shared/ui/input";
import { Select } from "../../../../shared/ui/select";
import { Textarea } from "../../../../shared/ui/textarea";
import { asInputBinding, SmartInput } from "../../forms/binding-inputs";
import { localizedExitLabel } from "../../model/node-display";
import { RequiredLabel } from "../../forms/required-label";
import { groupColor } from "../../nodes/node-appearance";
import type {
  ExitNodeData,
  JsonSchemaProperty,
  ReferenceCatalog,
  ReferenceNamespace,
  WorkflowCompletion,
  WorkflowEnd,
  WorkflowOutput,
  InputBinding,
} from "../../model/types";
import {
  ArtifactConstraints,
  INPUT_TYPES,
  internalNameError,
  nextKey,
  outputSchemaKind,
  outputType,
  SchemaShapeEditor,
  SectionTitle,
  asObject,
  schemaForInputKind,
} from "../interface-fields";

type ValueUpdater<T> = T | ((current: T) => T);

const ERROR_FIELDS = [
  ["code", "string"], ["message", "string"], ["details", "object"],
  ["sourceNodeId", "string"], ["nodeExecutionId", "string"], ["retryable", "boolean"],
] as const;

const COMPLETION_MODES: WorkflowCompletion[] = ["first_return", "all_complete"];

export function ExitPanel({
  exitId,
  end,
  exits,
  onEndChange,
  onExitUpdate,
  onClose,
  referenceCatalog,
  errorReferenceCatalog,
  readOnly = false,
}: {
  exitId: string;
  end: WorkflowEnd;
  exits: Array<{ id: string; data: ExitNodeData }>;
  onEndChange: (value: ValueUpdater<WorkflowEnd>) => void;
  onExitUpdate: (id: string, patch: Partial<ExitNodeData>) => void;
  onClose: () => void;
  referenceCatalog?: ReferenceCatalog;
  errorReferenceCatalog?: ReferenceCatalog;
  readOnly?: boolean;
}) {
  const { t } = useTranslation();
  const current = exits.find((exit) => exit.id === exitId);
  const error = end.error ?? { outputs: {} };
  const errorCatalog = errorReferenceCatalog ? { ...errorReferenceCatalog, item: [{ id: "item.json", label: t("studio.references.currentError"), path: "item.json", selector: { namespace: "item" as const, run: { kind: "current" as const }, item: { kind: "current" as const }, path: [] }, type: "object", schema: { type: "object" }, children: ERROR_FIELDS.map(([name, type]) => ({ id: `item.json.${name}`, label: t(`studio.errorFields.${name}`, name), path: `item.json.${name}`, selector: { namespace: "item" as const, run: { kind: "current" as const }, item: { kind: "current" as const }, path: [name] }, type, schema: { type }, children: [] })) }] } : undefined;
  const renameExitMapping = (group: "outputs" | "errorOutputs") => (from: string, to: string) => {
    for (const exit of exits) {
      const mappings = exit.data.parameters[group];
      if (!(from in mappings)) continue;
      const next = { ...mappings };
      next[to] = next[from];
      delete next[from];
      onExitUpdate(exit.id, { parameters: { ...exit.data.parameters, [group]: next } });
    }
  };
  const removeExitMapping = (group: "outputs" | "errorOutputs") => (name: string) => {
    for (const exit of exits) {
      const mappings = exit.data.parameters[group];
      if (!(name in mappings)) continue;
      const next = { ...mappings };
      delete next[name];
      onExitUpdate(exit.id, { parameters: { ...exit.data.parameters, [group]: next } });
    }
  };
  return (
    <InspectorShell testId="exit-panel">
      <header className="flex h-16 items-center gap-3 border-b border-border px-4">
        <span className="grid size-9 place-items-center rounded-md" style={{ color: groupColor('output'), backgroundColor: `color-mix(in srgb, ${groupColor('output')} 12%, transparent)` }}><LogOut className="size-5" /></span>
        <div className="min-w-0 flex-1"><strong className="block text-sm">{current ? localizedExitLabel(current.data.label, t("studio.exit.defaultName")) : t("studio.exit.title")}</strong><span className="text-[10px] text-muted-foreground">{t("studio.exit.description")}</span></div>
        <Button aria-label={t("common.close")} onClick={onClose} size="icon" variant="ghost"><X className="size-4" /></Button>
      </header>
      <ReadOnlyForm readOnly={readOnly} className="min-h-0 min-w-0 flex-1 space-y-6 overflow-y-auto p-4">
        <div className="rounded-md border border-primary/30 bg-primary/5 px-3 py-2 text-[10px] leading-4 text-primary" data-testid="exit-shared-banner">{t("studio.exit.sharedBanner")}</div>
        <ContractSection
          catalog={referenceCatalog}
          description={`${t("studio.exit.contractDescription")} ${t("studio.exit.mappingDescription")}`}
          mappings={current?.data.parameters.outputs ?? {}}
          outputs={end.outputs}
          title={t("studio.interface.successOutputs")}
          onRenameMapping={renameExitMapping("outputs")}
          onRemoveMapping={removeExitMapping("outputs")}
          onMappingChange={(name, value) => current && onExitUpdate(exitId, { parameters: { ...current.data.parameters, outputs: { ...current.data.parameters.outputs, [name]: value } } })}
          onSave={(update) => onEndChange((currentEnd) => ({ ...currentEnd, outputs: update(currentEnd.outputs) }))}
        />
        <ContractSection
          catalog={errorCatalog}
          description={`${t("studio.exit.errorContractDescription")} ${t("studio.exit.errorMappingDescription")}`}
          mappings={current?.data.parameters.errorOutputs ?? {}}
          outputs={error.outputs}
          title={t("studio.interface.errorFields")}
          onRenameMapping={renameExitMapping("errorOutputs")}
          onRemoveMapping={removeExitMapping("errorOutputs")}
          onMappingChange={(name, value) => current && onExitUpdate(exitId, { parameters: { ...current.data.parameters, errorOutputs: { ...current.data.parameters.errorOutputs, [name]: value } } })}
          onSave={(update) => onEndChange((currentEnd) => { const currentError = normalizedError(currentEnd.error); return { ...currentEnd, error: { ...currentError, outputs: update(currentError.outputs) } }; })}
        />
        <CompletionMode completion={end.completion} onChange={(completion) => onEndChange((currentEnd) => ({ ...currentEnd, completion }))} />
      </ReadOnlyForm>
    </InspectorShell>
  );
}

function CompletionMode({ completion, onChange }: { completion: WorkflowCompletion; onChange: (completion: WorkflowCompletion) => void }) {
  const { t } = useTranslation();
  return (
    <section>
      <SectionTitle description={t("studio.exit.completionDescription")} title={t("studio.exit.completion")} />
      <div className="grid grid-cols-1 gap-2">
        {COMPLETION_MODES.map((mode) => (
          <button
            className={`rounded-md border p-3 text-left transition-colors ${completion === mode ? "border-primary bg-primary/5 ring-1 ring-primary" : "border-border hover:border-primary/40"}`}
            data-testid={`completion-${mode}`}
            key={mode}
            onClick={() => onChange(mode)}
            type="button"
          >
            <span className="block text-xs font-semibold">{t(`studio.exit.completionModes.${mode}`)}</span>
            <span className="mt-1 block text-[10px] leading-4 text-muted-foreground">{t(`studio.exit.completionModeDescriptions.${mode}`)}</span>
          </button>
        ))}
      </div>
    </section>
  );
}

function ContractSection({ title, description, outputs, mappings, catalog, onSave, onRenameMapping, onRemoveMapping, onMappingChange }: { title: string; description?: string; outputs: Record<string, WorkflowOutput>; mappings: Record<string, InputBinding>; catalog?: ReferenceCatalog; onSave: (update: (current: Record<string, WorkflowOutput>) => Record<string, WorkflowOutput>) => void; onRenameMapping: (from: string, to: string) => void; onRemoveMapping: (name: string) => void; onMappingChange: (name: string, value: InputBinding) => void }) {
  const { t } = useTranslation();
  const [dialogOpen, setDialogOpen] = useState(false);
  const [editing, setEditing] = useState<{ name: string; originalName: string; original: WorkflowOutput; isNew: boolean } | null>(null);
  const add = () => {
    const name = nextKey(outputs, "output");
    setEditing({ name, originalName: name, original: { schema: { type: "string" }, required: false, sensitive: false }, isNew: true });
    setDialogOpen(true);
  };
  const edit = (name: string, output: WorkflowOutput) => {
    setEditing({ name, originalName: name, original: structuredClone(output), isNew: false });
    setDialogOpen(true);
  };
  const remove = (name: string) => { onSave((current) => { const next = { ...current }; delete next[name]; return next; }); onRemoveMapping(name); };
  const save = (name: string, output: WorkflowOutput) => {
    if (!editing) return;
    onSave((current) => {
      const next = { ...current };
      if (!editing.isNew && editing.originalName !== name) delete next[editing.originalName];
      next[name] = output;
      return next;
    });
    if (!editing.isNew && editing.originalName !== name) onRenameMapping(editing.originalName, name);
    setEditing(null);
    setDialogOpen(false);
  };
  return <section>
    <SectionTitle action={<Button onClick={add} size="sm" variant="secondary"><Plus className="size-3.5" />{t("studio.addField")}</Button>} description={description} title={title} />
    {Object.keys(outputs).length === 0 && <p className="rounded-md border border-dashed border-border px-3 py-4 text-center text-[10px] text-muted-foreground">{t("studio.exit.emptyContract")}</p>}
    <div className="space-y-2">{Object.entries(outputs).map(([name, output]) => <ContractSummary catalog={catalog} key={name} mapping={mappings[name]} name={name} output={output} onDelete={() => remove(name)} onEdit={() => edit(name, output)} onMappingChange={(value) => onMappingChange(name, value)} />)}</div>
    {editing && <ContractFieldDialog editing={editing} existing={outputs} onOpenChange={(open) => { if (!open) { setEditing(null); setDialogOpen(false); } }} onSave={save} open={dialogOpen} />}
  </section>;
}

function ContractSummary({ name, output, mapping, catalog, onMappingChange, onEdit, onDelete }: { name: string; output: WorkflowOutput; mapping?: InputBinding; catalog?: ReferenceCatalog; onMappingChange: (value: InputBinding) => void; onEdit: () => void; onDelete: () => void }) {
  const { t } = useTranslation();
  const schema = asObject(output.schema);
  const allowed: ReferenceNamespace[] = ["inputs", "outputs", "contexts", "execution"];
  if (catalog?.item?.length) allowed.push("item");
  return <div className="rounded-md border border-border px-3 py-2.5" data-testid={`exit-mapping-${name}`}><div className="mb-2 flex items-center gap-3"><div className="min-w-0 flex-1"><div className="flex items-center gap-2"><span className="truncate text-xs font-medium">{String(schema.title ?? name)}</span><span className="rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground">{t(`studio.schemaTypes.${outputSchemaKind(output.schema)}`, outputSchemaKind(output.schema))}</span>{output.required && <span className="text-[10px] text-danger">{t("studio.interface.required")}</span>}{output.sensitive && <span className="text-[10px] text-warning">{t("studio.interface.sensitive")}</span>}</div><div className="truncate font-mono text-[10px] text-muted-foreground">{name}</div></div><Button aria-label={t("studio.editField")} onClick={onEdit} size="icon" variant="ghost"><Pencil className="size-3.5" /></Button><Button aria-label={t("studio.removeField")} onClick={onDelete} size="icon" variant="ghost"><Trash2 className="size-3.5" /></Button></div><SmartInput allowedNamespaces={allowed} catalog={catalog} expectedSchema={output.schema as JsonSchemaProperty} onChange={onMappingChange} value={asInputBinding(mapping ?? "")} /></div>;
}

function ContractFieldDialog({ open, onOpenChange, editing, existing, onSave }: { open: boolean; onOpenChange: (open: boolean) => void; editing: { name: string; originalName: string; original: WorkflowOutput; isNew: boolean }; existing: Record<string, WorkflowOutput>; onSave: (name: string, output: WorkflowOutput) => void }) {
  const { t } = useTranslation();
  const [name, setName] = useState("");
  const [output, setOutput] = useState<WorkflowOutput>({ schema: { type: "string" }, required: false, sensitive: false });
  useEffect(() => {
    if (open) { setName(editing.name); setOutput(editing.original); }
  }, [open, editing]);
  const update = (patch: Partial<WorkflowOutput>) => setOutput((current) => ({ ...current, ...patch }));
  const updateType = (type: string) => update({ schema: schemaForInputKind(type, asObject(output.schema) as never) });
  const schema = asObject(output.schema);
  const nameError = internalNameError(name, existing, editing.originalName, t);
  return <Dialog onOpenChange={onOpenChange} open={open}><DialogContent description={t("studio.exit.contractDialogDescription")} title={t("studio.interface.outputField")}><div className="space-y-4 p-5"><div className="grid gap-3"><label className="text-xs"><span className="mb-1 block text-muted-foreground"><RequiredLabel required>{t("studio.interface.fieldType")}</RequiredLabel></span><Select aria-label={t("studio.interface.fieldType")} onValueChange={updateType} options={INPUT_TYPES.map((value) => ({ value, label: t(`studio.schemaTypes.${value}`, value) }))} value={outputSchemaKind(output.schema)} /></label><label className="text-xs"><span className="mb-1 block text-muted-foreground"><RequiredLabel required>{t("studio.interface.outputName")}</RequiredLabel></span><Input aria-invalid={Boolean(nameError)} aria-label={t("studio.interface.outputName")} onChange={(event) => setName(event.target.value)} value={name} />{nameError && <span className="mt-1 block text-[10px] text-danger" role="alert">{nameError}</span>}</label><label className="text-xs"><span className="mb-1 block text-muted-foreground">{t("studio.interface.fieldTitle")}</span><Input onChange={(event) => update({ schema: { ...schema, title: event.target.value } })} value={String(schema.title ?? "")} /></label>{!schema["x-agentx-artifact"] && (outputType(output.schema) === "object" || outputType(output.schema) === "array") && <SchemaShapeEditor onChange={(next) => update({ schema: next })} schema={output.schema as JsonSchemaProperty} />}{schema["x-agentx-artifact"] && <ArtifactConstraints onChange={(patch) => update({ schema: { ...schema, ...patch } })} property={schema as never} t={(key, fallback) => (fallback ? t(key, { defaultValue: fallback }) : t(key))} />}<div className="grid grid-cols-2 gap-3 text-xs"><label className="flex items-center gap-2"><input checked={output.required} className="size-4 accent-primary" onChange={(event) => update({ required: event.target.checked })} type="checkbox" />{t("studio.interface.required")}</label><label className="flex items-center gap-2"><input checked={output.sensitive} className="size-4 accent-primary" onChange={(event) => update({ sensitive: event.target.checked })} type="checkbox" />{t("studio.interface.sensitive")}</label></div><label className="text-xs"><span className="mb-1 block text-muted-foreground">{t("studio.interface.description")}</span><Textarea className="min-h-14" onChange={(event) => update({ schema: { ...schema, description: event.target.value } })} value={String(schema.description ?? "")} /></label></div><p className="text-[10px] text-muted-foreground">{t("studio.exit.contractRenameHint")}</p><div className="flex justify-end gap-2 border-t border-border pt-4"><Button onClick={() => onOpenChange(false)} variant="ghost">{t("common.cancel")}</Button><Button disabled={Boolean(nameError)} onClick={() => onSave(name.trim(), output)}>{t("common.save")}</Button></div></div></DialogContent></Dialog>;
}

function normalizedError(error: WorkflowEnd["error"] | undefined): NonNullable<WorkflowEnd["error"]> { return error ?? { outputs: {} }; }
