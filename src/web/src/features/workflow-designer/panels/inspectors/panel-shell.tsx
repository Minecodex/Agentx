import { Plus, Settings2, Trash2 } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";

import { Button } from "../../../../shared/ui/button";
import { Dialog, DialogContent } from "../../../../shared/ui/dialog";
import { Input } from "../../../../shared/ui/input";
import { Select } from "../../../../shared/ui/select";
import { asInputBinding, SmartInput } from "../../forms/binding-inputs";
import { ParameterField } from "../../forms/parameter-field";
import { RequiredLabel } from "../../forms/required-label";
import { ResourcePicker } from "../../forms/resource-picker";
import type { LocalizedManifest } from "../../model/manifest-localization";
import type {
  ActionNodeData,
  JsonSchemaProperty,
  NodeManifest,
  ReferenceCatalog,
  ResourceOption,
  ResourceRequestContext,
  ResourceType,
  UiField,
  InputBinding,
} from "../../model/types";
import { SectionTitle } from "../interface-fields";

export type ActionPanelProps = {
  data: ActionNodeData;
  manifest: NodeManifest;
  localized?: LocalizedManifest;
  fieldErrors: Record<string, string>;
  providerOptions: Record<string, ResourceOption[]>;
  referenceCatalog?: ReferenceCatalog;
  parameterCatalogs?: Record<string, ReferenceCatalog | undefined>;
  currentNodeCatalog?: ReferenceCatalog;
  resources: Partial<Record<ResourceType, ResourceOption[]>>;
  workflowId?: string;
  sourceNodeId?: string;
  readOnly?: boolean;
  onChange: (data: Partial<ActionNodeData>) => void;
  onResourceAuthorize?: (option: ResourceOption) => Promise<void>;
  onResourceRequest?: (option: ResourceOption, context?: ResourceRequestContext) => Promise<void>;
};

export function PanelSection({
  title,
  description,
  action,
  children,
  testId,
}: {
  title: string;
  description?: string;
  action?: React.ReactNode;
  children: React.ReactNode;
  testId?: string;
}) {
  return (
    <section data-testid={testId}>
      <SectionTitle action={action} description={description} title={title} />
      {children}
    </section>
  );
}

export function PanelHint({ children, testId }: { children: React.ReactNode; testId?: string }) {
  return (
    <div
      className="rounded-md border border-border bg-muted/30 px-3 py-2.5 text-[10px] leading-4 text-muted-foreground"
      data-testid={testId}
    >
      {children}
    </div>
  );
}

export function ModeCardsGroup({
  value,
  options,
  onChange,
}: {
  value: string;
  options: Array<{ value: string; label: string; description?: string }>;
  onChange: (value: string) => void;
}) {
  return (
    <div className="space-y-2" data-testid="mode-cards">
      {options.map((option) => {
        const active = value === option.value;
        return (
          <button
            className={`w-full rounded-md border p-3 text-left transition-colors ${active ? "border-primary bg-primary/5 ring-1 ring-primary" : "border-border hover:border-primary/40"}`}
            data-testid={`mode-card-${option.value}`}
            key={option.value}
            onClick={() => onChange(option.value)}
            type="button"
          >
            <span className="flex items-center gap-2 text-xs font-semibold">
              <span
                className={`grid size-3.5 shrink-0 place-items-center rounded-full border ${active ? "border-primary" : "border-muted-foreground/40"}`}
              >
                {active && <span className="size-1.5 rounded-full bg-primary" />}
              </span>
              {option.label}
            </span>
            {option.description && (
              <span className="mt-1 block text-[10px] leading-4 text-muted-foreground">
                {option.description}
              </span>
            )}
          </button>
        );
      })}
    </div>
  );
}

export function OutputContractHint({ manifest }: { manifest: NodeManifest }) {
  const { t } = useTranslation();
  const raw = (manifest.outputSchema as { properties?: Record<string, JsonSchemaProperty> } | undefined)?.properties ?? {};
  const structured = raw.structuredOutput;
  const structuredFields = structured?.properties ? Object.entries(structured.properties) : [];
  const diagnosticNames = new Set(["stdout", "stderr", "exitCode", "files", "partial", "reasoningContent", "citations", "usage", "finishReason"]);
  const native = Object.entries(raw).filter(([name]) => name !== "structuredOutput");
  const properties = [...native.filter(([name]) => !diagnosticNames.has(name)), ...structuredFields];
  const diagnostics = native.filter(([name]) => diagnosticNames.has(name));
  if (!structuredFields.length && structured) properties.push(["structuredOutput", structured]);
  if (properties.length === 0) return null;
  return (
    <div
      className="space-y-1.5 rounded-md border border-border bg-muted/30 px-3 py-2.5"
      data-testid="output-contract-hint"
    >
      {properties.map(([name, schema]) => (
        <div className="flex items-baseline gap-2 text-[10px]" key={name}>
          <code className="shrink-0 font-mono font-medium text-foreground">{name}</code>
          <span className="text-muted-foreground">{schemaTypeLabel(schema.type)}</span>
        </div>
      ))}
      {diagnostics.length > 0 && <details className="pt-1"><summary className="cursor-pointer text-[10px] text-muted-foreground">{t("studio.panels.code.diagnostics")}</summary><div className="mt-1 space-y-1">{diagnostics.map(([name, schema]) => <div className="flex items-baseline gap-2 text-[10px]" key={name}><code>{name}</code><span className="text-muted-foreground">{schemaTypeLabel(schema.type)}</span></div>)}</div></details>}
    </div>
  );
}

function schemaTypeLabel(type: unknown): string {
  if (Array.isArray(type)) return type.map((item) => String(item)).join(" | ");
  return type ? String(type) : "unknown";
}

export function ParameterControl({
  panel,
  name,
  unitOverride,
}: {
  panel: ActionPanelProps;
  name: string;
  unitOverride?: string;
}) {
  const schema = panel.manifest.parameterSchema.properties?.[name];
  if (!schema) return null;
  const ui: UiField | undefined = panel.manifest.uiSchema.fields?.[name];
  return (
    <div data-field-path={`parameters.${name}`}>
      <ParameterField
        enumLabels={enumLabelsFor(panel, name, schema)}
        error={panel.fieldErrors[`parameters.${name}`]}
        name={name}
        onChange={(value) =>
          panel.onChange({ parameters: { [name]: value } })
        }
        parameters={panel.data.parameters}
        providerOptions={panel.providerOptions[name]}
        referenceCatalog={panel.parameterCatalogs?.[name] ?? panel.referenceCatalog}
        required={panel.manifest.parameterSchema.required?.includes(name)}
        schema={schema}
        ui={ui}
        value={panel.data.parameters[name] ?? schema.default}
        workflowId={panel.workflowId}
        labelOverride={panel.localized?.parameterLabel(name)}
        descriptionOverride={panel.localized?.parameterDescription(name)}
        unitOverride={unitOverride}
        nestedLocalization={
          panel.localized
            ? {
                label: panel.localized.parameterLabel,
                description: panel.localized.parameterDescription,
                placeholder: panel.localized.parameterPlaceholder,
                enumLabel: panel.localized.parameterEnumLabel,
              }
            : undefined
        }
      />
    </div>
  );
}

function enumLabelsFor(panel: ActionPanelProps, name: string, schema: JsonSchemaProperty) {
  return Object.fromEntries(
    (schema.enum ?? []).map((option) => [
      String(option),
      panel.localized?.parameterEnumLabel(name, String(option)) ?? String(option),
    ]),
  );
}

export function ResourceSelectorFields({ panel }: { panel: ActionPanelProps }) {
  const { t } = useTranslation();
  return <>{resourceSelectors(panel.manifest).filter(selector => !selector.bindingRole).map(selector => {
    const selected = panel.data.resourceReferences.filter(reference => !reference.bindingRole && reference.resourceType === selector.resourceType);
    const options = resourceOptionsFor(panel.resources, selector.resourceType, selector.operation);
    return <div className="space-y-2" key={`${selector.resourceType}-${selector.operation}`}>
      {selector.multiple && selected.map(reference => <div className="flex items-center justify-between rounded-md border border-border px-3 py-2 text-xs" key={reference.resourceId}>
        <span>{options.find(option => option.value === reference.resourceId)?.metadata?.connectionName} / {options.find(option => option.value === reference.resourceId)?.label ?? reference.resourceId}</span>
        <Button aria-label={t("studio.inspector.clearResource")} size="icon" variant="ghost" onClick={() => panel.onChange({ resourceReferences: panel.data.resourceReferences.filter(item => item !== reference) })}><Trash2 className="size-3" /></Button>
      </div>)}
      <ResourceSelect fieldPath="resourceReferences" label={selector.resourceType === "rag" ? t("knowledge.queries") : selector.label ?? selector.resourceType.replaceAll("_", " ")}
        onChange={(resourceId, versionId) => {
          if (selector.multiple && selected.some(reference => reference.resourceId === resourceId)) return;
          panel.onChange({ resourceReferences: [
            ...panel.data.resourceReferences.filter(reference => selector.multiple || reference.bindingRole || reference.resourceType !== selector.resourceType),
            { resourceType: selector.resourceType, resourceId, resourceVersionId: versionId, operation: selector.operation },
          ] });
        }}
        options={options} onAuthorize={panel.onResourceAuthorize} onRequest={panel.onResourceRequest} sourceNodeId={panel.sourceNodeId}
        required={selector.required} optionsMissing={Boolean(selector.required && !options.length)} testId={`resource-selector-${selector.resourceType}`}
        value={selector.multiple ? undefined : selected[0]?.resourceId} />
    </div>;
  })}</>;
}

export function BindingSlotResourceFields({ panel }: { panel: ActionPanelProps }) {
  const selectors = resourceSelectors(panel.manifest);
  return <>{panel.manifest.bindingSlots.map((slot) => {
    const selector = selectors.find((candidate) => candidate.bindingRole === slot.name);
    const operation = selector?.operation ?? "use";
    const reference = panel.data.resourceReferences.find((candidate) => candidate.bindingRole === slot.name);
    return <ResourceSelect
      error={panel.fieldErrors[`resourceReferences.${slot.name}`]}
      fieldPath={`resourceReferences.${slot.name}`}
      key={slot.name}
      label={panel.localized?.bindingSlotLabel(slot.name) ?? selector?.label ?? slot.name.replaceAll("_", " ")}
      onAuthorize={panel.onResourceAuthorize}
      onChange={(resourceId, versionId) => panel.onChange({ resourceReferences: [
        ...panel.data.resourceReferences.filter((candidate) => candidate.bindingRole !== slot.name),
        { bindingRole: slot.name, resourceType: slot.resourceType, resourceId, resourceVersionId: versionId, operation },
      ] })}
      onClear={slot.required ? undefined : () => panel.onChange({ resourceReferences: panel.data.resourceReferences.filter((candidate) => candidate.bindingRole !== slot.name) })}
      onRequest={panel.onResourceRequest}
      options={resourceOptionsFor(panel.resources, slot.resourceType, operation)}
      required={slot.required}
      sourceNodeId={panel.sourceNodeId}
      testId={`binding-slot-${slot.name}`}
      value={reference?.resourceId}
    />;
  })}</>;
}

export function CommonPanelSections({ panel }: { panel: ActionPanelProps }) {
  const { t } = useTranslation();
  if (!panel.manifest.contextWriteCapability) return null;
  return (
    <details className="rounded-md border border-border/70">
      <summary className="cursor-pointer px-3 py-2 text-xs font-medium text-muted-foreground">
        {t("studio.inspector.advancedConfiguration")}
      </summary>
      <div className="border-t border-border p-3">
        <ContextWritesConfiguration
          label={t("studio.inspector.contextWrites")}
          onChange={(contextWrites) =>
            panel.onChange({
              contextWrites: (Array.isArray(contextWrites) ? contextWrites : []) as ActionNodeData["contextWrites"],
            })
          }
          value={panel.data.contextWrites}
          referenceCatalog={panel.currentNodeCatalog}
        />
      </div>
    </details>
  );
}

function ContextWritesConfiguration({
  label,
  value,
  onChange,
  referenceCatalog,
}: {
  label: string;
  value: unknown;
  onChange: (value: unknown) => void;
  referenceCatalog?: ReferenceCatalog;
}) {
  const { t } = useTranslation();
  const writes = Array.isArray(value)
    ? (value as Array<{ operation?: string; path?: string; value?: InputBinding }>)
    : [];
  const [dialogOpen, setDialogOpen] = useState(false);
  const [editingIndex, setEditingIndex] = useState<number | null>(null);
  const [draft, setDraft] = useState<ContextWriteDraft>({
    operation: "set",
    path: "",
    value: { kind: "literal", value: "" },
  });
  const add = () => {
    setEditingIndex(null);
    setDraft({ operation: "set", path: "", value: { kind: "literal", value: "" } });
    setDialogOpen(true);
  };
  const edit = (index: number) => {
    setEditingIndex(index);
    setDraft({
      operation: writes[index].operation ?? "set",
      path: writes[index].path ?? "",
      value: writes[index].value ?? { kind: "literal", value: "" },
    });
    setDialogOpen(true);
  };
  const save = () => {
    onChange(
      editingIndex === null
        ? [...writes, draft]
        : writes.map((write, index) => (index === editingIndex ? draft : write)),
    );
    setDialogOpen(false);
  };
  return (
    <section>
      <div className="mb-2 flex items-center justify-between gap-3">
        <span className="text-xs text-muted-foreground">{label}</span>
        <Button onClick={add} size="sm" variant="secondary">
          <Plus className="size-3.5" />
          {t("studio.inspector.addContextWrite")}
        </Button>
      </div>
      <div className="space-y-2">
        {writes.map((write, index) => (
          <div
            className="flex items-center gap-3 rounded-md border border-border px-3 py-2.5"
            key={`${write.path ?? "context"}-${index}`}
          >
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-2">
                <code className="truncate text-xs font-medium">
                  {write.path || t("studio.inspector.contextPath")}
                </code>
                <span className="rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground">
                  {t(`studio.contextOperations.${write.operation ?? "set"}`, write.operation ?? "set")}
                </span>
              </div>
              {write.operation !== "delete" && write.value && (
                <div className="truncate text-[10px] text-muted-foreground">
                  {inputBindingSummary(write.value)}
                </div>
              )}
            </div>
            <Button
              aria-label={t("studio.inspector.editContextWrite")}
              onClick={() => edit(index)}
              size="icon"
              variant="ghost"
            >
              <Settings2 className="size-3.5" />
            </Button>
            <Button
              aria-label={t("studio.removeField")}
              onClick={() => onChange(writes.filter((_, current) => current !== index))}
              size="icon"
              variant="ghost"
            >
              <Trash2 className="size-3.5" />
            </Button>
          </div>
        ))}
      </div>
      <ContextWriteDialog
        draft={draft}
        onChange={setDraft}
        onClose={() => setDialogOpen(false)}
        onSave={save}
        open={dialogOpen}
        referenceCatalog={referenceCatalog}
        editing={editingIndex !== null}
      />
    </section>
  );
}

type ContextWriteDraft = { operation: string; path: string; value: InputBinding };

function inputBindingSummary(value: InputBinding): string {
  switch (value.kind) {
    case "literal": return typeof value.value === "string" ? value.value : JSON.stringify(value.value);
    case "reference": return [value.selector.namespace, value.selector.port, ...(value.selector.path ?? [])].filter(Boolean).join(" / ");
    case "template": return value.segments.map((segment) => segment.kind === "text" ? segment.text : `{{${segment.selector.namespace}.${(segment.selector.path ?? []).join(".")}}}`).join("");
    case "array": return `[${value.items.length}]`;
    case "object": return `{${Object.keys(value.fields).length}}`;
  }
}

function ContextWriteDialog({
  open,
  editing,
  draft,
  referenceCatalog,
  onChange,
  onClose,
  onSave,
}: {
  open: boolean;
  editing: boolean;
  draft: ContextWriteDraft;
  referenceCatalog?: ReferenceCatalog;
  onChange: (value: ContextWriteDraft) => void;
  onClose: () => void;
  onSave: () => void;
}) {
  const { t } = useTranslation();
  const variableOptions = globalVariableOptions(referenceCatalog?.contexts ?? []);
  const selectVariable = (path: string) => {
    const operations = contextOperationsFor(referenceCatalog, path);
    onChange({
      ...draft,
      path,
      operation: operations.includes(draft.operation) ? draft.operation : "set",
    });
  };
  return (
    <Dialog onOpenChange={(next) => !next && onClose()} open={open}>
      <DialogContent
        description={t("studio.inspector.contextWriteDialogDescription")}
        title={editing ? t("studio.inspector.editContextWrite") : t("studio.inspector.addContextWrite")}
      >
        <div className="space-y-4 p-5">
          <div>
            <h2 className="text-sm font-semibold">
              {editing ? t("studio.inspector.editContextWrite") : t("studio.inspector.addContextWrite")}
            </h2>
            <p className="mt-1 text-[11px] text-muted-foreground">
              {t("studio.inspector.contextWriteDialogDescription")}
            </p>
          </div>
          <div className="grid gap-3">
            <label className="text-xs">
              <span className="mb-1 block text-muted-foreground">
                <RequiredLabel required>{t("studio.inspector.contextPath")}</RequiredLabel>
              </span>
              <Select
                aria-label={t("studio.inspector.contextPath")}
                disabled={variableOptions.length === 0}
                onValueChange={selectVariable}
                options={variableOptions}
                placeholder={t("studio.inspector.selectContext")}
                value={draft.path}
              />
            </label>
            <label className="text-xs">
              <span className="mb-1 block text-muted-foreground">
                <RequiredLabel required>{t("studio.inspector.contextOperation")}</RequiredLabel>
              </span>
              <Select
                aria-label={t("studio.inspector.contextOperation")}
                disabled={!draft.path}
                onValueChange={(operation) => onChange({ ...draft, operation })}
                options={contextOperationsFor(referenceCatalog, draft.path).map((operation) => ({
                  value: operation,
                  label: t(`studio.contextOperations.${operation}`, operation),
                }))}
                value={draft.operation}
              />
            </label>
            <label className="text-xs">
              <span className="mb-1 block text-muted-foreground">
                <RequiredLabel required={draft.operation !== "delete"}>
                  {t("studio.inspector.contextValue")}
                </RequiredLabel>
              </span>
              {draft.operation === "delete" ? (
                <Input aria-label={t("studio.inspector.contextValue")} disabled value="" />
              ) : (
                <SmartInput
                  allowedNamespaces={["inputs", "outputs", "contexts", "execution"]}
                  catalog={referenceCatalog}
                  expectedSchema={findReference(referenceCatalog?.contexts ?? [], `contexts.${draft.path}`)?.schema}
                  onChange={(value) => onChange({ ...draft, value })}
                  value={asInputBinding(draft.value)}
                />
              )}
            </label>
          </div>
          <div className="flex justify-end gap-2 border-t border-border pt-4">
            <Button onClick={onClose} variant="ghost">
              {t("common.cancel")}
            </Button>
            <Button disabled={!draft.path.trim()} onClick={onSave}>
              {t("common.save")}
            </Button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}

function globalVariableOptions(
  entries: ReferenceCatalog["contexts"],
): Array<{ value: string; label: string }> {
  return entries.flatMap((entry) => {
    const value = entry.path.replace(/^contexts\./, "");
    return [{ value, label: value }, ...globalVariableOptions(entry.children)];
  });
}

function contextOperationsFor(catalog: ReferenceCatalog | undefined, path: string | undefined) {
  const common = ["set", "set_if_absent", "compare_and_set", "delete"];
  const target = findReference(catalog?.contexts ?? [], `contexts.${path ?? ""}`);
  if (target?.type === "array") return [...common.slice(0, 2), "append", ...common.slice(2)];
  if (target?.type === "object")
    return [...common.slice(0, 2), "merge_object", ...common.slice(2)];
  if (target?.type === "number" || target?.type === "integer")
    return [...common.slice(0, 2), "increment", "min", "max", ...common.slice(2)];
  return common;
}

function findReference(
  entries: ReferenceCatalog["contexts"],
  path: string,
): ReferenceCatalog["contexts"][number] | undefined {
  for (const entry of entries) {
    if (entry.path === path) return entry;
    const nested = findReference(entry.children, path);
    if (nested) return nested;
  }
  return undefined;
}

export function ResourceSelect({
  label,
  options,
  value,
  required,
  optionsMissing,
  testId,
  fieldPath,
  onChange,
  onClear,
  error,
  onAuthorize,
  onRequest,
  sourceNodeId,
}: {
  label: string;
  options: ResourceOption[];
  value?: string;
  required?: boolean;
  optionsMissing?: boolean;
  testId?: string;
  fieldPath?: string;
  onChange: (id: string, versionId?: string | null, label?: string) => void;
  onClear?: () => void;
  error?: string;
  onAuthorize?: (option: ResourceOption) => Promise<void>;
  onRequest?: (option: ResourceOption, context?: ResourceRequestContext) => Promise<void>;
  sourceNodeId?: string;
}) {
  const { t } = useTranslation();
  return (
    <PanelField
      error={error}
      fieldPath={fieldPath}
      label={label}
      required={required}
      testId={testId}
    >
      <ResourcePicker
        onAuthorize={onAuthorize}
        onChange={onChange}
        onRequest={
          onRequest ? (option, message) => onRequest(option, { sourceNodeId, message }) : undefined
        }
        options={options}
        value={value}
      />
      {onClear && value && (
        <Button className="mt-1.5" onClick={onClear} size="sm" variant="ghost">
          {t("studio.inspector.clearResource")}
        </Button>
      )}
      {optionsMissing && (
        <span className="mt-1 block text-[10px] text-warning">{t("studio.inspector.noResource")}</span>
      )}
    </PanelField>
  );
}

export function PanelField({
  children,
  label,
  required,
  error,
  testId,
  fieldPath,
}: {
  children: React.ReactNode;
  label: string;
  required?: boolean;
  error?: string;
  testId?: string;
  fieldPath?: string;
}) {
  return (
    <label className="block text-xs" data-field-path={fieldPath} data-testid={testId}>
      <span className="mb-1.5 block text-muted-foreground">
        <RequiredLabel required={required}>{label}</RequiredLabel>
      </span>
      {children}
      {error && <span className="mt-1 block text-[10px] text-danger">{error}</span>}
    </label>
  );
}

type ResourceSelector = {
  multiple?: boolean;
  bindingRole?: string;
  resourceType: ResourceType;
  operation: "view" | "use" | "read" | "write" | "manage";
  required?: boolean;
  label?: string;
};

export function resourceSelectors(manifest: NodeManifest): ResourceSelector[] {
  const value = manifest.uiSchema.resourceSelectors;
  if (!Array.isArray(value)) return [];
  return value.filter((item): item is ResourceSelector =>
    Boolean(
      item &&
        typeof item === "object" &&
        typeof (item as ResourceSelector).resourceType === "string" &&
        typeof (item as ResourceSelector).operation === "string",
    ),
  );
}

export function resourceOptionsFor(
  resources: Partial<Record<ResourceType, ResourceOption[]>>,
  resourceType: ResourceType,
  operation: ResourceOption["operation"],
) {
  return (resources[resourceType] ?? []).filter(
    (option) => option.operation === undefined || option.operation === operation,
  );
}
