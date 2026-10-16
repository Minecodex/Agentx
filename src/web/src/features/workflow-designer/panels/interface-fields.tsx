import { Pencil, Plus, Trash2 } from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { Button } from "../../../shared/ui/button";
import { Dialog, DialogContent } from "../../../shared/ui/dialog";
import { Input } from "../../../shared/ui/input";
import { Select } from "../../../shared/ui/select";
import { Textarea } from "../../../shared/ui/textarea";
import { StructuredJsonControl } from "../forms/parameter-field";
import { RequiredLabel } from "../forms/required-label";
import type {
  ContextDefinition,
  WorkflowStart,
  JsonSchemaProperty,
} from "../model/types";
import { isReferenceKey } from "../utils/definition-validation";

export type ValueUpdater<T> = T | ((current: T) => T);

export type InputProperty = JsonSchemaProperty & {
  "x-agentx-artifact"?: boolean;
  "x-agentx-artifact-array"?: boolean;
  "x-agentx-max-size-bytes"?: number;
  "x-agentx-max-total-size-bytes"?: number;
  "x-agentx-content-types"?: string[];
};

export const INPUT_TYPES = ["string", "number", "boolean", "object", "array", "artifact", "artifact_array"];

export function SectionTitle({ title, description, action }: { title: string; description?: string; action?: React.ReactNode }) {
  return (
    <div className="mb-3 flex items-start justify-between gap-3">
      <div>
        <h2 className="text-xs font-semibold">{title}</h2>
        {description && <p className="mt-1 text-[10px] text-muted-foreground">{description}</p>}
      </div>
      {action}
    </div>
  );
}

export function FieldSummary({ name, title, description, type, required, onEdit, onDelete }: { name: string; title?: string; description?: string; type: string; required?: boolean; onEdit: () => void; onDelete: () => void }) {
  const { t } = useTranslation();
  return (
    <div className="flex items-center gap-3 rounded-md border border-border px-3 py-2.5">
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <span className="truncate text-xs font-medium">{title || name}</span>
          <span className="rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground">{t(`studio.schemaTypes.${type}`, type)}{required ? " *" : ""}</span>
        </div>
        <div className="truncate font-mono text-[10px] text-muted-foreground">{name}</div>
        {description && <div className="truncate text-[10px] text-muted-foreground">{description}</div>}
      </div>
      <Button aria-label={t("studio.editField")} onClick={onEdit} size="icon" variant="ghost"><Pencil className="size-3.5" /></Button>
      <Button aria-label={t("studio.removeField")} onClick={onDelete} size="icon" variant="ghost"><Trash2 className="size-3.5" /></Button>
    </div>
  );
}

export function LabeledNumberInput({ label, value, min, onChange }: { label: string; value: unknown; min?: number; onChange: (value: number | undefined) => void }) {
  return <label className="text-xs"><span className="mb-1 block text-muted-foreground">{label}</span><Input aria-label={label} min={min} onChange={(event) => onChange(optionalNumber(event.target.value))} type="number" value={numberValue(value)} /></label>;
}

function TypeSpecificSettings({ property, onChange, t }: { property: InputProperty; onChange: (patch: Partial<InputProperty>) => void; t: (key: string, fallback?: string) => string }) {
  const type = schemaType(property);
  const isArray = type === "array";
  const isObject = type === "object";
  return (
    <div className="grid gap-3">
      <DefaultControl onChange={(defaultValue) => onChange({ default: defaultValue })} property={property} t={t} />
      {isObject && <label className="flex items-center gap-2 text-xs"><input checked={property.additionalProperties !== false} className="size-4 accent-primary" onChange={(event) => onChange({ additionalProperties: event.target.checked })} type="checkbox" />{t("studio.interface.additionalProperties")}</label>}
      {isArray && <ArrayConstraintFields onChange={onChange} property={property} t={t} />}
      {!isArray && !isObject && <ConstraintFields onChange={onChange} property={property} t={t} />}
    </div>
  );
}

function DefaultControl({ property, onChange, t }: { property: InputProperty; onChange: (value: unknown) => void; t: (key: string, fallback?: string) => string }) {
  const type = schemaType(property);
  if (type === "boolean") return <label className="text-xs"><span className="mb-1 block text-muted-foreground">{t("studio.interface.default")}</span><span className="flex h-9 items-center gap-2 rounded-lg border border-border bg-surface px-3"><input checked={Boolean(property.default)} className="size-4 accent-primary" onChange={(event) => onChange(event.target.checked)} type="checkbox" />{t("studio.interface.defaultEnabled")}</span></label>;
  if (type === "object" || type === "array") return <div className="text-xs"><span className="mb-1 block text-muted-foreground">{t("studio.interface.default")}</span><StructuredJsonControl fallback={type === "array" ? [] : {}} onChange={onChange} schema={property} value={property.default} /></div>;
  return <label className="text-xs"><span className="mb-1 block text-muted-foreground">{t("studio.interface.default")}</span><Input aria-label={t("studio.interface.default")} onChange={(event) => onChange(parseScalar(event.target.value, type))} value={property.default === undefined ? "" : String(property.default)} /></label>;
}

function ConstraintFields({ property, onChange, t }: { property: InputProperty; onChange: (patch: Partial<InputProperty>) => void; t: (key: string, fallback?: string) => string }) {
  const type = schemaType(property);
  const isString = type === "string" && !property["x-agentx-artifact"];
  const isNumber = type === "number" || type === "integer";
  const supportsEnum = isString || isNumber;
  return (
    <div className="grid grid-cols-2 gap-2">
      {isString && <label className="col-span-2 text-xs"><span className="mb-1 block text-muted-foreground">{t("studio.interface.format")}</span><Select aria-label={t("studio.interface.format")} onValueChange={(value) => onChange({ format: value === "none" ? undefined : value })} options={["none", "email", "uri", "date", "date-time", "uuid"].map((value) => ({ value, label: t(`studio.formats.${value}`, value) }))} value={property.format ?? "none"} /></label>}
      {isString && <>
        <LabeledNumberInput label={t("studio.interface.minLength")} min={0} onChange={(value) => onChange({ minLength: value })} value={property.minLength} />
        <LabeledNumberInput label={t("studio.interface.maxLength")} min={0} onChange={(value) => onChange({ maxLength: value })} value={property.maxLength} />
        <label className="col-span-2 text-xs"><span className="mb-1 block text-muted-foreground">{t("studio.interface.pattern")}</span><Input aria-label={t("studio.interface.pattern")} onChange={(event) => onChange({ pattern: event.target.value || undefined })} placeholder="^[a-z0-9_]+$" value={property.pattern ?? ""} /></label>
      </>}
      {isNumber && <>
        <LabeledNumberInput label={t("studio.interface.minimum")} onChange={(value) => onChange({ minimum: value })} value={property.minimum} />
        <LabeledNumberInput label={t("studio.interface.maximum")} onChange={(value) => onChange({ maximum: value })} value={property.maximum} />
        <LabeledNumberInput label={t("studio.interface.multipleOf")} min={0} onChange={(value) => onChange({ multipleOf: value })} value={property.multipleOf} />
      </>}
      {supportsEnum && <label className="col-span-2 text-xs"><span className="mb-1 block text-muted-foreground">{t("studio.interface.enum")}</span><Input aria-label={t("studio.interface.enum")} onChange={(event) => onChange({ enum: parseEnum(event.target.value, type) })} value={(property.enum ?? []).join(", ")} /></label>}
    </div>
  );
}

function ArrayConstraintFields({ property, onChange, t }: { property: InputProperty; onChange: (patch: Partial<InputProperty>) => void; t: (key: string, fallback?: string) => string }) {
  return (
    <div className="grid grid-cols-2 gap-2">
      <LabeledNumberInput label={t("studio.interface.minItems")} min={0} onChange={(value) => onChange({ minItems: value })} value={property.minItems} />
      <LabeledNumberInput label={t("studio.interface.maxItems")} min={0} onChange={(value) => onChange({ maxItems: value })} value={property.maxItems} />
      <label className="col-span-2 flex items-center gap-2 text-xs"><input checked={Boolean(property.uniqueItems)} className="size-4 accent-primary" onChange={(event) => onChange({ uniqueItems: event.target.checked || undefined })} type="checkbox" />{t("studio.interface.uniqueItems")}</label>
    </div>
  );
}

export function SchemaShapeEditor({ schema, onChange }: { schema: JsonSchemaProperty; onChange: (patch: JsonSchemaProperty) => void }) {
  const { t } = useTranslation();
  const translate = (key: string, fallback?: string) => fallback ? t(key, { defaultValue: fallback }) : t(key);
  const properties = schema.properties ?? {};
  const updateProperty = (name: string, patch: Partial<JsonSchemaProperty>) => onChange({ ...schema, properties: { ...properties, [name]: { ...properties[name], ...patch } } });
  const removeProperty = (name: string) => { const next = { ...properties }; delete next[name]; onChange({ ...schema, properties: next, required: (schema.required ?? []).filter((item) => item !== name) }); };
  const addProperty = () => { const name = nextKey(properties, "field"); onChange({ ...schema, type: "object", additionalProperties: false, properties: { ...properties, [name]: { type: "string", title: name } } }); };
  const toggleRequired = (name: string, checked: boolean) => onChange({ ...schema, required: checked ? [...new Set([...(schema.required ?? []), name])] : (schema.required ?? []).filter((item) => item !== name) });
  if (schema.type === "array") return <div className="mt-2 rounded-md border border-border/60 p-2"><div className="mb-2 text-[10px] font-medium text-muted-foreground">{t("studio.interface.itemSchema")}</div><Select aria-label={t("studio.interface.itemType")} onValueChange={(type) => onChange({ ...schema, items: schemaForType(type) })} options={INPUT_TYPES.slice(0, 5).map((value) => ({ value, label: t(`studio.schemaTypes.${value}`, value) }))} value={String(schema.items?.type ?? "string")} />{(schema.items?.type === "object" || schema.items?.type === "array") && <SchemaShapeEditor onChange={(items) => onChange({ ...schema, items })} schema={schema.items} />}{schema.items && <details className="mt-2 rounded border border-border/50 px-2 py-1.5"><summary className="cursor-pointer text-[10px] font-medium text-muted-foreground">{t("studio.interface.constraintsAndDefault")}</summary><div className="mt-2"><TypeSpecificSettings onChange={(patch) => onChange({ ...schema, items: { ...schema.items, ...patch } })} property={schema.items} t={translate} /></div></details>}</div>;
  if (schema.type !== "object") return null;
  return (
    <div className="mt-2 space-y-2 rounded-md border border-border/60 p-2">
      <div className="text-[10px] font-medium text-muted-foreground">{t("studio.interface.objectSchema")}</div>
      {Object.entries(properties).map(([name, property]) => (
        <div className="rounded border border-border/50 p-2" key={name}>
          <div className="grid grid-cols-[minmax(0,1fr)_110px_32px] gap-2">
            <Input aria-label={t("studio.interface.nestedFieldName")} defaultValue={name} onBlur={(event) => { const next = event.target.value.trim(); if (!next || next === name || properties[next]) return; const nextProperties = { ...properties, [next]: property }; delete nextProperties[name]; onChange({ ...schema, properties: nextProperties, required: (schema.required ?? []).map((item) => item === name ? next : item) }); }} />
            <Select aria-label={t("studio.interface.fieldType")} onValueChange={(type) => updateProperty(name, schemaForType(type))} options={INPUT_TYPES.slice(0, 5).map((value) => ({ value, label: t(`studio.schemaTypes.${value}`, value) }))} value={String(property.type ?? "string")} />
            <Button aria-label={t("studio.removeField")} onClick={() => removeProperty(name)} size="icon" variant="ghost"><Trash2 className="size-3.5" /></Button>
          </div>
          {(property.type === "object" || property.type === "array") && <SchemaShapeEditor onChange={(next) => updateProperty(name, next)} schema={property} />}
          <label className="mt-2 flex items-center gap-2 text-xs"><input checked={(schema.required ?? []).includes(name)} className="size-4 accent-primary" onChange={(event) => toggleRequired(name, event.target.checked)} type="checkbox" />{t("studio.interface.required")}</label>
          <details className="mt-2 rounded border border-border/50 px-2 py-1.5"><summary className="cursor-pointer text-[10px] font-medium text-muted-foreground">{t("studio.interface.constraintsAndDefault")}</summary><div className="mt-2"><TypeSpecificSettings onChange={(patch) => updateProperty(name, patch)} property={property} t={translate} /></div></details>
        </div>
      ))}
      <Button aria-label={t("studio.addField")} onClick={addProperty} size="sm" variant="ghost"><Plus className="size-3.5" />{t("studio.addField")}</Button>
    </div>
  );
}

export function ArtifactConstraints({ property, onChange, t }: { property: InputProperty; onChange: (patch: Partial<InputProperty>) => void; t: (key: string, fallback?: string) => string }) {
  const multiple = Boolean(property["x-agentx-artifact-array"]);
  return (
    <div className="mt-2 grid gap-2 rounded-md border border-border/60 p-3">
      <div className="text-[10px] font-medium text-muted-foreground">{t("studio.interface.fileConstraints")}</div>
      <label className="text-xs"><span className="mb-1 block text-muted-foreground">{t("studio.interface.contentTypes")}</span><Input aria-label={t("studio.interface.contentTypes")} onChange={(event) => onChange({ "x-agentx-content-types": event.target.value.split(",").map((item) => item.trim()).filter(Boolean) })} placeholder="application/pdf, image/*" value={(property["x-agentx-content-types"] ?? []).join(", ")} /></label>
      <label className="text-xs"><span className="mb-1 block text-muted-foreground">{t("studio.interface.maxSizeBytes")}</span><Input aria-label={t("studio.interface.maxSizeBytes")} min={1} onChange={(event) => onChange({ "x-agentx-max-size-bytes": optionalNumber(event.target.value) })} type="number" value={numberValue(property["x-agentx-max-size-bytes"])} /></label>
      {multiple && <>
        <div className="grid grid-cols-2 gap-2"><LabeledNumberInput label={t("studio.interface.minFiles")} min={0} onChange={(minItems) => onChange({ minItems })} value={property.minItems} /><LabeledNumberInput label={t("studio.interface.maxFiles")} min={0} onChange={(maxItems) => onChange({ maxItems })} value={property.maxItems} /></div>
        <label className="text-xs"><span className="mb-1 block text-muted-foreground">{t("studio.interface.maxTotalSizeBytes")}</span><Input aria-label={t("studio.interface.maxTotalSizeBytes")} min={1} onChange={(event) => onChange({ "x-agentx-max-total-size-bytes": optionalNumber(event.target.value) })} type="number" value={numberValue(property["x-agentx-max-total-size-bytes"])} /></label>
      </>}
    </div>
  );
}

export function InputFieldDialog({ open, onOpenChange, editing, properties, onSave }: { open: boolean; onOpenChange: (open: boolean) => void; editing: { name: string; property: InputProperty; required: boolean } | null; properties: Record<string, InputProperty>; onSave: (name: string, property: InputProperty, required: boolean) => void }) {
  const { t } = useTranslation();
  const [name, setName] = useState("");
  const [property, setProperty] = useState<InputProperty>({ type: "string", title: "" });
  const [required, setRequired] = useState(false);
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const translate = (key: string, fallback?: string) => fallback ? t(key, { defaultValue: fallback }) : t(key);
  const setKind = (value: string) => setProperty((current) => schemaForInputKind(value, current));
  useEffect(() => {
    if (open) {
      setName(editing?.name ?? nextKey(properties, "input"));
      setProperty(editing?.property ?? { type: "string", title: "" });
      setRequired(editing?.required ?? false);
      setAdvancedOpen(false);
    }
  }, [open, editing, properties]);
  const kind = inputKind(property);
  const showAdvanced = kind !== "artifact" && kind !== "artifact_array";
  const propertyError = schemaConstraintError(property, translate);
  const nameError = fieldNameError(name, properties, editing?.name, t);
  return (
    <Dialog onOpenChange={onOpenChange} open={open}>
      <DialogContent description={t("studio.interface.fieldDialogDescription")} title={editing ? t("studio.editField") : t("studio.addField")}>
        <div className="space-y-4 p-5">
          <div>
            <h2 className="text-sm font-semibold">{editing ? t("studio.editField") : t("studio.addField")}</h2>
            <p className="mt-1 text-[11px] text-muted-foreground">{t("studio.interface.fieldDialogDescription")}</p>
          </div>
          <div className="grid gap-3">
            <label className="text-xs"><span className="mb-1 block text-muted-foreground"><RequiredLabel required>{t("studio.interface.fieldType")}</RequiredLabel></span><Select onValueChange={setKind} options={INPUT_TYPES.map((value) => ({ value, label: t(`studio.schemaTypes.${value}`, value) }))} value={kind} /></label>
            <label className="text-xs"><span className="mb-1 block text-muted-foreground"><RequiredLabel required>{t("studio.interface.fieldName")}</RequiredLabel></span><Input aria-invalid={Boolean(nameError)} onChange={(e) => setName(e.target.value)} value={name} />{nameError && <span className="mt-1 block text-[10px] text-danger" role="alert">{nameError}</span>}</label>
            <label className="text-xs"><span className="mb-1 block text-muted-foreground">{t("studio.interface.fieldTitle")}</span><Input onChange={(e) => setProperty((p) => ({ ...p, title: e.target.value }))} value={property.title ?? ""} /></label>
            <label className="text-xs"><span className="mb-1 block text-muted-foreground">{t("studio.interface.description")}</span><Textarea className="min-h-14" onChange={(e) => setProperty((p) => ({ ...p, description: e.target.value }))} value={property.description ?? ""} /></label>
            <label className="flex items-center gap-2 text-xs"><input checked={required} className="size-4 accent-primary" onChange={(e) => setRequired(e.target.checked)} type="checkbox" />{t("studio.interface.required")}</label>
            {(kind === "object" || kind === "array") && <SchemaShapeEditor onChange={(next) => setProperty(next as InputProperty)} schema={property} />}
            {(kind === "artifact" || kind === "artifact_array") && <ArtifactConstraints onChange={(patch) => setProperty((p) => ({ ...p, ...patch }))} property={property} t={translate} />}
            {showAdvanced && <details className="rounded-md border border-border/70 px-3 py-2" onToggle={(event) => setAdvancedOpen(event.currentTarget.open)} open={advancedOpen}><summary className="cursor-pointer text-xs font-medium text-muted-foreground">{t("studio.interface.constraintsAndDefault")}</summary><div className="mt-3"><TypeSpecificSettings onChange={(patch) => setProperty((p) => ({ ...p, ...patch }))} property={property} t={translate} /></div></details>}
          </div>
          {propertyError && <p className="text-xs text-danger" role="alert">{propertyError}</p>}
          <div className="flex justify-end gap-2 border-t border-border pt-4"><Button onClick={() => onOpenChange(false)} variant="ghost">{t("common.cancel")}</Button><Button disabled={Boolean(nameError || propertyError)} onClick={() => onSave(name.trim(), property, required)}>{t("common.save")}</Button></div>
        </div>
      </DialogContent>
    </Dialog>
  );
}

export function ContextForm({ contexts, onChange }: { contexts: WorkflowStart["contexts"]; onChange: (value: ValueUpdater<WorkflowStart["contexts"]>) => void }) {
  const { t } = useTranslation();
  const [dialogOpen, setDialogOpen] = useState(false);
  const [editing, setEditing] = useState<{ name: string; context: ContextDefinition } | null>(null);
  const add = () => { setEditing(null); setDialogOpen(true); };
  const save = (name: string, context: ContextDefinition) => onChange((current) => { const next = { ...current }; if (editing?.name && editing.name !== name) delete next[editing.name]; next[name] = context; return next; });
  const remove = (name: string) => onChange((current) => { const next = { ...current }; delete next[name]; return next; });
  return (
    <section>
      <SectionTitle
        action={<Button onClick={add} size="sm" variant="secondary"><Plus className="size-3.5" />{t("studio.addContext")}</Button>}
        description={t("studio.interface.contextsDescription")}
        title={t("studio.interface.contexts")}
      />
      <div className="space-y-2">{Object.entries(contexts).map(([name, context]) => <FieldSummary key={name} name={name} description={context.description ?? undefined} title={context.title ?? undefined} type={String((context.schema as JsonSchemaProperty).type ?? "string")} onDelete={() => remove(name)} onEdit={() => { setEditing({ name, context }); setDialogOpen(true); }} />)}</div>
      <ContextFieldDialog editing={editing} onOpenChange={setDialogOpen} onSave={(name, context) => { save(name, context); setDialogOpen(false); }} open={dialogOpen} contexts={contexts} />
    </section>
  );
}

function ContextFieldDialog({ open, onOpenChange, editing, contexts, onSave }: { open: boolean; onOpenChange: (open: boolean) => void; editing: { name: string; context: ContextDefinition } | null; contexts: WorkflowStart["contexts"]; onSave: (name: string, context: ContextDefinition) => void }) {
  const { t } = useTranslation();
  const [name, setName] = useState("");
  const [context, setContext] = useState<ContextDefinition>({ schema: { type: "string" }, default: "", mutable: true, sensitive: false, clientWritable: false, scope: "execution_tree", mergePolicy: "replace", maxSize: null });
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const schema = context.schema as JsonSchemaProperty;
  const schemaWithDefault = { ...schema, default: context.default };
  const type = String(schema.type ?? "string");
  const translate = (key: string, fallback?: string) => fallback ? t(key, { defaultValue: fallback }) : t(key);
  useEffect(() => { if (open) { setName(editing?.name ?? nextKey(contexts, "variable")); setContext(editing?.context ?? { schema: { type: "string" }, default: "", mutable: true, sensitive: false, clientWritable: false, scope: "execution_tree", mergePolicy: "replace", maxSize: null }); setAdvancedOpen(false); } }, [open, editing, contexts]);
  const setType = (nextType: string) => setContext((current) => ({ ...current, schema: schemaForType(nextType), default: defaultForType(nextType, current.default), mergePolicy: contextMergePolicies(nextType).includes(current.mergePolicy) ? current.mergePolicy : "replace" }));
  const nameError = internalNameError(name, contexts, editing?.name, t);
  const maxSizeError = context.maxSize != null && context.maxSize <= 0 ? t("studio.interface.invalidPositiveNumber") : undefined;
  return (
    <Dialog onOpenChange={onOpenChange} open={open}>
      <DialogContent description={t("studio.interface.contextDialogDescription")} title={editing ? t("studio.editContext") : t("studio.addContext")}>
        <div className="space-y-4 p-5">
          <div>
            <h2 className="text-sm font-semibold">{editing ? t("studio.editContext") : t("studio.addContext")}</h2>
            <p className="mt-1 text-[11px] text-muted-foreground">{t("studio.interface.contextDialogDescription")}</p>
          </div>
          <div className="grid gap-3">
            <label className="text-xs"><span className="mb-1 block text-muted-foreground"><RequiredLabel required>{t("studio.interface.fieldType")}</RequiredLabel></span><Select onValueChange={setType} options={INPUT_TYPES.slice(0, 5).map((value) => ({ value, label: t(`studio.schemaTypes.${value}`, value) }))} value={type} /></label>
            <label className="text-xs"><span className="mb-1 block text-muted-foreground"><RequiredLabel required>{t("studio.interface.contextName")}</RequiredLabel></span><Input aria-invalid={Boolean(nameError)} onChange={(e) => setName(e.target.value)} value={name} />{nameError && <span className="mt-1 block text-[10px] text-danger" role="alert">{nameError}</span>}</label>
            <label className="text-xs"><span className="mb-1 block text-muted-foreground">{t("studio.interface.fieldTitle")}</span><Input onChange={(e) => setContext((c) => ({ ...c, title: e.target.value }))} value={context.title ?? ""} /></label>
            <label className="text-xs"><span className="mb-1 block text-muted-foreground">{t("studio.interface.description")}</span><Textarea className="min-h-16" onChange={(e) => setContext((c) => ({ ...c, description: e.target.value }))} value={context.description ?? ""} /></label>
            <DefaultControl onChange={(value) => setContext((c) => ({ ...c, default: value }))} property={schemaWithDefault} t={translate} />
            <label className="flex items-center gap-2 text-xs"><input checked={context.sensitive} className="size-4 accent-primary" onChange={(e) => setContext((c) => ({ ...c, sensitive: e.target.checked }))} type="checkbox" />{t("studio.interface.sensitive")}</label>
            {(type === "object" || type === "array") && <SchemaShapeEditor onChange={(next) => setContext((c) => ({ ...c, schema: next }))} schema={schema} />}
            <details className="rounded-md border border-border/70 px-3 py-2" onToggle={(event) => setAdvancedOpen(event.currentTarget.open)} open={advancedOpen}>
              <summary className="cursor-pointer text-xs font-medium text-muted-foreground">{t("studio.interface.advanced")}</summary>
              <div className="mt-3 grid gap-3">
                <div className="grid grid-cols-2 gap-2 text-xs">
                  <label className="flex items-center gap-2"><input checked={context.mutable} className="size-4 accent-primary" onChange={(e) => setContext((c) => ({ ...c, mutable: e.target.checked }))} type="checkbox" />{t("studio.interface.mutable")}</label>
                  <label className="flex items-center gap-2"><input checked={context.clientWritable} className="size-4 accent-primary" onChange={(e) => setContext((c) => ({ ...c, clientWritable: e.target.checked }))} type="checkbox" />{t("studio.interface.clientWritable")}</label>
                </div>
                <div className="grid grid-cols-2 gap-2">
                  <label className="text-xs"><span className="mb-1 block text-muted-foreground"><RequiredLabel required>{t("studio.interface.scope")}</RequiredLabel></span><Select onValueChange={(value) => setContext((c) => ({ ...c, scope: value as ContextDefinition["scope"] }))} options={[{ value: "execution_tree", label: t("studio.contextScopes.execution_tree") }, { value: "session", label: t("studio.contextScopes.session") }]} value={context.scope} /></label>
                  <label className="text-xs"><span className="mb-1 block text-muted-foreground"><RequiredLabel required>{t("studio.interface.mergePolicy")}</RequiredLabel></span><Select onValueChange={(value) => setContext((c) => ({ ...c, mergePolicy: value as ContextDefinition["mergePolicy"] }))} options={contextMergePolicies(type).map((value) => ({ value, label: t(`studio.mergePolicies.${value}`, value) }))} value={context.mergePolicy} /></label>
                </div>
                <LabeledNumberInput label={t("studio.interface.maxSize")} min={1} onChange={(maxSize) => setContext((c) => ({ ...c, maxSize }))} value={context.maxSize} />
                {maxSizeError && <span className="text-[10px] text-danger" role="alert">{maxSizeError}</span>}
                {type === "array" ? <ArrayConstraintFields onChange={(patch) => setContext((c) => ({ ...c, schema: { ...schema, ...patch } }))} property={schema} t={translate} /> : type === "object" ? <label className="flex items-center gap-2 text-xs"><input checked={schema.additionalProperties !== false} className="size-4 accent-primary" onChange={(event) => setContext((c) => ({ ...c, schema: { ...schema, additionalProperties: event.target.checked } }))} type="checkbox" />{t("studio.interface.additionalProperties")}</label> : <ConstraintFields onChange={(patch) => setContext((c) => ({ ...c, schema: { ...schema, ...patch } }))} property={schema} t={translate} />}
              </div>
            </details>
          </div>
          <div className="flex justify-end gap-2 border-t border-border pt-4"><Button onClick={() => onOpenChange(false)} variant="ghost">{t("common.cancel")}</Button><Button disabled={Boolean(nameError || maxSizeError)} onClick={() => onSave(name.trim(), context)}>{t("common.save")}</Button></div>
        </div>
      </DialogContent>
    </Dialog>
  );
}

export function asObject(value: unknown): Record<string, any> { return value && typeof value === "object" && !Array.isArray(value) ? value as Record<string, any> : {}; }
function asProperties(value: unknown): Record<string, InputProperty> { return asObject(value) as Record<string, InputProperty>; }
export function nextKey(value: Record<string, unknown>, prefix: string) { let index = Object.keys(value).length + 1; while (value[`${prefix}_${index}`]) index += 1; return `${prefix}_${index}`; }
export function inputKind(property: InputProperty) { if (property["x-agentx-artifact-array"]) return "artifact_array"; if (property["x-agentx-artifact"]) return "artifact"; return schemaType(property); }
function parseScalar(value: string, type?: string): unknown { if (!value) return type === "string" ? "" : undefined; if (type === "number") return Number(value); if (type === "boolean") return value === "true"; return value; }
function parseEnum(value: string, type?: string) { return value.split(",").map((item) => parseScalar(item.trim(), type)).filter((item) => item !== undefined); }
function optionalNumber(value: string) { return value.trim() ? Number(value) : undefined; }
function numberValue(value: unknown) { return typeof value === "number" ? String(value) : ""; }
export function outputType(schema: unknown) { return schemaType(asObject(schema) as JsonSchemaProperty); }
export function outputSchemaKind(schema: unknown) { return inputKind(asObject(schema) as InputProperty); }
function schemaType(schema: JsonSchemaProperty): string { return Array.isArray(schema.type) ? schema.type.find((type) => type !== "null") ?? schema.type[0] ?? "string" : schema.type ?? "string"; }
function schemaForType(type: string): JsonSchemaProperty { return type === "object" ? { type, additionalProperties: false, properties: {} } : type === "array" ? { type, items: { type: "string" } } : { type }; }
function artifactReferenceSchema(): JsonSchemaProperty { return { type: "object", required: ["artifactId"], properties: { artifactId: { type: "string", format: "uuid" }, type: { type: "string" }, fileName: { type: "string" }, contentType: { type: "string" }, sizeBytes: { type: "number", minimum: 0 }, sha256: { type: "string" } }, additionalProperties: false }; }
export function schemaForInputKind(kind: string, current: InputProperty): InputProperty {
  const metadata = { title: current.title, description: current.description };
  if (kind === "artifact") return { ...artifactReferenceSchema(), ...metadata, format: "artifact-reference", "x-agentx-artifact": true, "x-agentx-artifact-array": false };
  if (kind === "artifact_array") return { type: "array", items: artifactReferenceSchema(), ...metadata, "x-agentx-artifact": true, "x-agentx-artifact-array": true };
  return { ...schemaForType(kind), ...metadata };
}
function defaultForType(type: string, current: unknown) { if (type === "object") return {}; if (type === "array") return []; if (type === "boolean") return Boolean(current); return parseScalar(String(current ?? ""), type); }
function contextMergePolicies(type: string): ContextDefinition["mergePolicy"][] {
  if (type === "array") return ["replace", "append", "reject_conflict"];
  if (type === "object") return ["replace", "merge_object", "reject_conflict"];
  if (type === "number") return ["replace", "increment", "reject_conflict"];
  return ["replace", "reject_conflict"];
}
function schemaConstraintError(property: InputProperty, t: (key: string, fallback?: string) => string): string | undefined {
  const invalidRange = (minimum: unknown, maximum: unknown) => typeof minimum === "number" && typeof maximum === "number" && minimum > maximum;
  if (invalidRange(property.minimum, property.maximum) || invalidRange(property.minLength, property.maxLength) || invalidRange(property.minItems, property.maxItems)) return t("studio.interface.invalidRange");
  if (typeof property.multipleOf === "number" && property.multipleOf <= 0) return t("studio.interface.invalidStep");
  if (typeof property.pattern === "string") { try { new RegExp(property.pattern); } catch { return t("studio.interface.invalidPattern"); } }
  if (typeof property["x-agentx-max-size-bytes"] === "number" && property["x-agentx-max-size-bytes"]! <= 0) return t("studio.interface.invalidFileSize");
  if (typeof property["x-agentx-max-total-size-bytes"] === "number" && property["x-agentx-max-total-size-bytes"]! <= 0) return t("studio.interface.invalidFileSize");
  for (const child of Object.values(property.properties ?? {})) { const error = schemaConstraintError(child as InputProperty, t); if (error) return error; }
  if (property.items) return schemaConstraintError(property.items as InputProperty, t);
  return undefined;
}
function fieldNameError(name: string, existing: Record<string, unknown>, originalName: string | undefined, t: ReturnType<typeof useTranslation>["t"]) { const clean = name.trim(); if (!clean) return t("studio.interface.nameRequired"); if (clean !== originalName && existing[clean]) return t("studio.interface.duplicateName"); return undefined; }
export function internalNameError(name: string, existing: Record<string, unknown>, originalName: string | undefined, t: ReturnType<typeof useTranslation>["t"]) { const clean = name.trim(); if (!clean) return t("studio.interface.nameRequired"); if (!isReferenceKey(clean)) return t("studio.interface.invalidInternalName"); if (clean !== originalName && existing[clean]) return t("studio.interface.duplicateName"); return undefined; }
export function asInputProperties(value: unknown): Record<string, InputProperty> { return asProperties(value); }
export function schemaRequired(schema: Record<string, unknown>) { return Array.isArray(schema.required) ? schema.required.map(String) : []; }
