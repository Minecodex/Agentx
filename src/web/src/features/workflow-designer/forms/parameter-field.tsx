import { AlertTriangle, Pencil, Plus, Trash2 } from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { Button } from "../../../shared/ui/button";
import { Input } from "../../../shared/ui/input";
import { Select } from "../../../shared/ui/select";
import type {
  JsonSchemaProperty,
  ReferenceCatalog,
  ReferenceNamespace,
  ResourceOption,
  UiField,
  ConditionOperator,
  ConditionSpec,
  InputBinding,
  ValueSelector,
} from "../model/types";
import { selectorsEqual } from "../model/selector";
import { CodeEditor } from "./code-editor";
import { asReferenceBinding, asInputBinding, ReferenceInput, TemplateInput, SmartInput } from "./binding-inputs";
import { RequiredLabel } from "./required-label";

const EMPTY_JSON_OBJECT = {};
const EMPTY_JSON_ARRAY: unknown[] = [];

export function ParameterField({
  name,
  schema,
  ui,
  value,
  required,
  error,
  parameters,
  providerOptions = [],
  workflowId,
  referenceCatalog,
  onChange,
  onValidityChange,
  labelOverride,
  descriptionOverride,
  enumLabels,
  nestedLocalization,
  unitOverride,
}: {
  name: string;
  schema: JsonSchemaProperty;
  ui?: UiField;
  value: unknown;
  required?: boolean;
  error?: string;
  parameters: Record<string, unknown>;
  providerOptions?: ResourceOption[];
  workflowId?: string;
  referenceCatalog?: ReferenceCatalog;
  onChange: (value: unknown) => void;
  onValidityChange?: (valid: boolean) => void;
  labelOverride?: string;
  descriptionOverride?: string;
  enumLabels?: Record<string, string>;
  nestedLocalization?: StructuredFieldLocalization;
  unitOverride?: string;
}) {
  const { t } = useTranslation();
  const label = labelOverride ?? ui?.label ?? schema.title ?? humanize(name);
  const visible =
    !ui?.visibleWhen ||
    parameters[ui.visibleWhen.field] === ui.visibleWhen.equals;
  const control = ui?.control;
  const bindingEnabled = Boolean(referenceCatalog) && Boolean(schema["x-agentx-binding"]);
  useEffect(() => {
    onValidityChange?.(control ? SUPPORTED_CONTROLS.has(control) : false);
  }, [control, onValidityChange]);
  if (!visible) return null;
  if (!control || !SUPPORTED_CONTROLS.has(control))
    return <UnsupportedControl label={label} control={control} />;
  const unitKey = ui?.unit ?? inferredUnit(name);
  const unit = unitOverride ?? (unitKey ? t(`studio.units.${unitKey}`, unitKey) : undefined);
  const field = (content: React.ReactNode) => (
    <Field
      error={error}
      label={label}
      required={required}
      testId={`parameter-${name}`}
      unit={unit}
    >
      {content}
      {descriptionOverride && <p className="mt-1 text-[10px] text-muted-foreground">{descriptionOverride}</p>}
    </Field>
  );
  if (control === "select")
    return field(
      <Select
        aria-label={label}
        className="w-full"
        onValueChange={onChange}
        options={(ui.options ?? schema.enum ?? []).map((item) =>
          selectOption(item, enumLabels?.[String(item)]),
        )}
        value={value === undefined ? "" : String(value)}
      />,
    );
  if (control === "provider_options")
    return field(
      <Select
        aria-label={label}
        className="w-full"
        onValueChange={onChange}
        options={providerOptions}
        value={value === undefined ? "" : String(value)}
      />,
    );
  if (control === "boolean")
    return (
      <div>
        <label className="flex items-center gap-2 text-xs">
          <input
            checked={Boolean(value)}
            className="size-4 accent-primary"
            onChange={(event) => onChange(event.target.checked)}
            type="checkbox"
          />
          <span><RequiredLabel required={required}>{label}</RequiredLabel></span>
        </label>
        {error && <p className="mt-1 text-[10px] text-danger">{error}</p>}
      </div>
    );
  if (control === "number")
    return field(
      <Input
        aria-label={label}
        max={schema.maximum}
        min={schema.minimum}
        onChange={(event) =>
          onChange(
            event.target.value === "" ? undefined : Number(event.target.value),
          )
        }
        step={schema.type === "integer" ? 1 : "any"}
        type="number"
        value={value === undefined ? "" : String(value)}
      />,
    );
  if (["template", "textarea", "prompt"].includes(control))
    return field(
      <TemplateInput
        allowedNamespaces={allowedNamespaces(schema)}
        catalog={referenceCatalog}
        multiline={control !== "template" || Boolean(schema.multiline)}
        onChange={onChange}
        value={asInputBinding(value)}
      />,
    );
  if (control === "reference")
    return field(
      <ReferenceInput
        allowedNamespaces={allowedNamespaces(schema)}
        catalog={referenceCatalog}
        expectedSchema={schema}
        onChange={onChange}
        value={asReferenceBinding(value)}
      />,
    );
  if (control === "value")
    return field(<SmartInput allowedNamespaces={allowedNamespaces(schema)} catalog={referenceCatalog} expectedSchema={schema} onChange={onChange} value={asInputBinding(value)} />);
  if (["structured", "json", "schema_editor", "kv_builder", "sort_builder", "api_key_placement"].includes(control))
    return field(
      <StructuredJsonControl
        catalog={referenceCatalog}
        enabled={bindingEnabled}
        fallback={
          schemaHasType(schema, "array") ? EMPTY_JSON_ARRAY : EMPTY_JSON_OBJECT
        }
        onChange={onChange}
        onValidityChange={onValidityChange}
        schema={schema}
        value={value}
        localization={nestedLocalization}
        path={name}
      />,
    );
  if (control === "code")
    return field(
      <CodeControl
        language={codeLanguage(
          ui.languageField ? parameters[ui.languageField] : undefined,
        )}
        onChange={onChange}
        value={String(value ?? "")}
      />,
    );
  if (control === "collection")
    return field(<CollectionControl catalog={referenceCatalog} enabled={bindingEnabled} itemSchema={schema.items} name={name} onChange={onChange} value={value} />);
  if (control === "fixed_collection")
    return field(<FixedCollectionControl catalog={referenceCatalog} enabled={bindingEnabled} onChange={onChange} value={value} />);
  if (control === "mapper")
    return field(<MapperControl catalog={referenceCatalog} enabled={bindingEnabled} onChange={onChange} schema={schema} value={value} />);
  if (control === "condition_builder")
    return field(<ConditionBuilderControl catalog={referenceCatalog} onChange={onChange} value={value} />);
  if (control === "buttons_editor")
    return field(<ButtonsEditorControl onChange={onChange} value={value} />);
  void workflowId;
  return field(<Input aria-label={label} onChange={(event) => onChange(event.target.value)} value={value === undefined || value === null ? "" : String(value)} />);
}

function CodeControl({
  language,
  value,
  onChange,
}: {
  language: string;
  value: string;
  onChange: (value: string) => void;
}) {
  const { t } = useTranslation();
  return (
    <div className="overflow-hidden rounded-md border border-border bg-canvas shadow-inner focus-within:border-primary focus-within:ring-2 focus-within:ring-primary/15">
      <div className="flex h-7 items-center justify-between border-b border-border px-2.5 text-[9px] font-medium uppercase tracking-wide text-muted-foreground">
        <span>{language}</span>
        <span>{t("studio.source")}</span>
      </div>
      <CodeEditor
        height="238px"
        language={language}
        onChange={onChange}
        value={value}
      />
    </div>
  );
}

export function StructuredJsonControl({
  value,
  fallback,
  onChange,
  onValidityChange,
  catalog,
  schema,
  enabled,
  localization,
  path = "value",
}: {
  value: unknown;
  fallback: unknown;
  onChange: (value: unknown) => void;
  onValidityChange?: (valid: boolean) => void;
  catalog?: ReferenceCatalog;
  schema: JsonSchemaProperty;
  enabled?: boolean;
  localization?: StructuredFieldLocalization;
  path?: string;
}) {
  useEffect(() => onValidityChange?.(true), [onValidityChange]);
  return <ObjectBuilder catalog={catalog} enabled={enabled} localization={localization} onChange={onChange} path={path} schema={schema} value={value ?? fallback} />;
}

export type StructuredFieldLocalization = {
  label: (path: string) => string;
  description: (path: string) => string | undefined;
  placeholder: (path: string) => string | undefined;
  enumLabel: (path: string, value: string) => string;
};

function ObjectBuilder({ schema, value, onChange, catalog, enabled, path, localization }: { schema: JsonSchemaProperty; value: unknown; onChange: (value: unknown) => void; catalog?: ReferenceCatalog; enabled?: boolean; path: string; localization?: StructuredFieldLocalization }) {
  const { t } = useTranslation();
  if (enabled) return <SmartInput allowedNamespaces={allowedNamespaces(schema)} catalog={catalog} expectedSchema={schema} onChange={onChange} value={asInputBinding(value)} />;
  if (!schema.type) {
    const inferred = jsonValueType(value);
    if (enabled && (inferred === "object" || inferred === "array")) {
      return <ObjectBuilder catalog={catalog} enabled localization={localization} onChange={onChange} path={path} schema={inferred === "object" ? { type: "object", additionalProperties: {} } : { type: "array", items: {} }} value={value} />;
    }
    return <AnyJsonValueBuilder catalog={catalog} enabled={enabled} localization={localization} onChange={onChange} path={path} schema={schema} value={value} />;
  }
  const type = primarySchemaType(schema);
  if (type === 'object') {
    const current = value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : {};
    const properties = schema.properties ?? {};
    const fixedKeys = Object.keys(properties);
    const dynamicKeys = Object.keys(current).filter((name) => !properties[name]);
    const dynamicSchema = typeof schema.additionalProperties === 'object' ? schema.additionalProperties : {};
    const set = (name: string, next: unknown) => onChange({ ...current, [name]: next });
    const remove = (name: string) => { const next = { ...current }; delete next[name]; onChange(next); };
    const rename = (name: string, nextName: string) => { const clean = nextName.trim(); if (!clean || clean === name || current[clean] !== undefined) return; const next = { ...current, [clean]: current[name] }; delete next[name]; onChange(next); };
    const add = () => { let index = dynamicKeys.length + 1; while (current[`field${index}`] !== undefined) index += 1; set(`field${index}`, defaultForSchema(dynamicSchema)); };
    return <div className="space-y-2 rounded-md border border-border/70 p-2" data-testid="json-object">{fixedKeys.map((name) => { const childPath = `${path}.${name}`; const childSchema = properties[name]; const label = localization?.label(childPath) ?? childSchema?.title ?? humanize(name); return <div data-field-path={childPath} key={name}>{childSchema?.type !== "boolean" && <span className="mb-1 block text-[10px] font-medium text-muted-foreground">{label}</span>}<ObjectBuilder catalog={catalog} enabled={enabled} localization={localization} onChange={(next) => set(name, next)} path={childPath} schema={childSchema} value={current[name]} />{localization?.description(childPath) && <p className="mt-1 text-[10px] text-muted-foreground">{localization.description(childPath)}</p>}</div>; })}{schema.additionalProperties !== false && dynamicKeys.map((name) => <div className="space-y-2 rounded-md border border-border/60 p-2" data-field-path={`${path}.${name}`} data-testid="json-object-field" key={name}><div className="flex items-center gap-2"><Input aria-label={t('studio.key')} className="min-w-0 flex-1" defaultValue={name} onBlur={(event) => rename(name, event.target.value)} /><Button aria-label={t('studio.removeField', { key: name })} onClick={() => remove(name)} size="icon" variant="ghost"><Trash2 className="size-3.5" /></Button></div><ObjectBuilder catalog={catalog} enabled={enabled} localization={localization} onChange={(next) => set(name, next)} path={`${path}.${name}`} schema={dynamicSchema} value={current[name]} /></div>)}{schema.additionalProperties !== false && <Button aria-label={t('studio.addField')} onClick={add} size="sm" variant="ghost"><Plus className="size-3.5" />{t('studio.addField')}</Button>}</div>;
  }
  if (type === 'array') {
    const items = Array.isArray(value) ? value : [];
    return <div className="space-y-2" data-testid="json-array">{items.map((item, index) => <div className="space-y-1 rounded-md border border-border/60 p-2" data-testid="json-array-item" key={index}><div className="flex items-center justify-between"><span className="text-[10px] text-muted-foreground">#{index + 1}</span><Button aria-label={t('studio.remove', { index: index + 1 })} onClick={() => onChange(items.filter((_, currentIndex) => currentIndex !== index))} size="icon" variant="ghost"><Trash2 className="size-3.5" /></Button></div><ObjectBuilder catalog={catalog} enabled={enabled} localization={localization} onChange={(next) => onChange(items.map((current, currentIndex) => currentIndex === index ? next : current))} path={`${path}[]`} schema={schema.items ?? {}} value={item} /></div>)}<Button aria-label={t('studio.addItem')} onClick={() => onChange([...items, defaultForSchema(schema.items)])} size="sm" variant="secondary"><Plus className="size-3.5" />{t('studio.addItem')}</Button></div>;
  }
  const label = localization?.label(path) ?? schema.title ?? humanize(path.split('.').at(-1)?.replace('[]', '') ?? path);
  const bindingEnabled = Boolean(enabled || schema["x-agentx-binding"]);
  if (bindingEnabled) return <SmartInput allowedNamespaces={allowedNamespaces(schema)} catalog={catalog} expectedSchema={schema} onChange={onChange} value={asInputBinding(value)} />;
  if (type === 'boolean') return <label className="flex items-center gap-2 text-xs"><input checked={Boolean(value)} className="size-4 accent-primary" onChange={(event) => onChange(event.target.checked)} type="checkbox" />{label}</label>;
  if (schema.enum?.length) return <Select aria-label={label} className="w-full" onValueChange={(next) => onChange(schema.enum?.find((option) => String(option) === next) ?? next)} options={schema.enum.map((option) => ({ value: String(option), label: localization?.enumLabel(path, String(option)) ?? String(option) }))} value={value === undefined ? "" : String(value)} />;
  return <Input aria-label={label || t('studio.value')} min={schema.minimum} onChange={(event) => onChange(type === 'number' || type === 'integer' ? Number(event.target.value) : event.target.value)} placeholder={localization?.placeholder(path)} type={type === 'number' || type === 'integer' ? 'number' : 'text'} value={value === undefined || value === null ? '' : String(value)} />;
}

function AnyJsonValueBuilder({ schema, value, onChange, catalog, enabled, path = "value", localization }: { schema: JsonSchemaProperty; value: unknown; onChange: (value: unknown) => void; catalog?: ReferenceCatalog; enabled?: boolean; path?: string; localization?: StructuredFieldLocalization }) {
  const { t } = useTranslation();
  if (enabled || schema["x-agentx-binding"]) return <SmartInput allowedNamespaces={allowedNamespaces(schema)} catalog={catalog} expectedSchema={schema} onChange={onChange} value={asInputBinding(value)} />;
  const type = jsonValueType(value);
  const typedSchema = { ...schema, type: type === 'null' ? 'string' : type } as JsonSchemaProperty;
  return <div className="space-y-1" data-testid="json-any-value"><Select aria-label={t('studio.jsonValueType')} className="w-full min-w-0" onValueChange={(next) => onChange(defaultForSchema({ type: next }))} options={JSON_VALUE_TYPES.map((item) => ({ value: item, label: t(`studio.schemaTypes.${item}`, item) }))} value={type === 'null' ? 'string' : type} /><ObjectBuilder catalog={catalog} enabled={enabled} localization={localization} onChange={onChange} path={path} schema={typedSchema} value={value === null || value === undefined ? defaultForSchema(typedSchema) : value} /></div>;
}

const JSON_VALUE_TYPES = ['string', 'number', 'boolean', 'object', 'array'] as const;
function jsonValueType(value: unknown): string {
  const unwrapped = isDynamicLiteral(value) ? value.value : value;
  if (Array.isArray(unwrapped)) return 'array';
  if (unwrapped && typeof unwrapped === 'object') return 'object';
  if (typeof unwrapped === 'number') return 'number';
  if (typeof unwrapped === 'boolean') return 'boolean';
  return 'string';
}

function isDynamicLiteral(value: unknown): value is Extract<InputBinding, { kind: 'literal' }> {
  return Boolean(value && typeof value === 'object' && (value as { kind?: unknown }).kind === 'literal' && 'value' in value);
}

function defaultForSchema(schema?: JsonSchemaProperty): unknown { if (schema?.default !== undefined) return schema.default; if (schemaHasType(schema, 'object')) return {}; if (schemaHasType(schema, 'array')) return []; if (schemaHasType(schema, 'boolean')) return false; if (schemaHasType(schema, 'number') || schemaHasType(schema, 'integer')) return 0; return ''; }

function CollectionControl({
  value,
  itemSchema,
  catalog,
  enabled,
  name,
  onChange,
}: {
  value: unknown;
  itemSchema?: JsonSchemaProperty;
  catalog?: ReferenceCatalog;
  enabled?: boolean;
  name: string;
  onChange: (value: unknown[]) => void;
}) {
  const { t } = useTranslation();
  const items = Array.isArray(value) ? value : [];
  const update = (index: number, next: unknown) =>
    onChange(items.map((item, current) => (current === index ? next : item)));
  return (
    <div className="space-y-2">
      {items.map((item, index) => (
        <div className="flex items-start gap-2" key={index}>
          <ObjectBuilder
            catalog={catalog}
            enabled={enabled}
            onChange={(next) => update(index, next)}
            path={`${name}[]`}
            schema={itemSchema ?? {}}
            value={item}
          />
          <Button
            aria-label={t("studio.remove", { index: index + 1 })}
            onClick={() =>
              onChange(items.filter((_, current) => current !== index))
            }
            size="icon"
            variant="ghost"
          >
            <Trash2 className="size-3.5" />
          </Button>
        </div>
      ))}
      <Button
        onClick={() =>
          onChange([...items, itemSchema?.type === "object" ? {} : ""])
        }
        size="sm"
        variant="secondary"
      >
        <Plus className="size-3.5" />
        {t("studio.addItem")}
      </Button>
    </div>
  );
}

function FixedCollectionControl({
  value,
  catalog,
  enabled,
  onChange,
}: {
  value: unknown;
  catalog?: ReferenceCatalog;
  enabled?: boolean;
  onChange: (value: Record<string, unknown>) => void;
}) {
  const { t } = useTranslation();
  const binding = asInputBinding(value);
  const fields = binding.kind === "object" ? binding.fields : {};
  const entries = Object.entries(fields);
  const emit = (next: Record<string, unknown>) => onChange({ kind: "object", fields: next } as unknown as Record<string, unknown>);
  const replace = (index: number, key: string, next: unknown) =>
    emit(
      Object.fromEntries(
        entries.map(([currentKey, currentValue], current) =>
          current === index ? [key, next] : [currentKey, currentValue],
        ),
      ),
    );
  return (
    <div className="space-y-2">
      {entries.map(([key, item], index) => (
        <div
          className="grid grid-cols-[minmax(0,1fr)_minmax(0,1fr)_32px] gap-2"
          key={index}
        >
          <Input
            aria-label={t("studio.key")}
            onChange={(event) => replace(index, event.target.value, item)}
            value={key}
          />
          <AnyJsonValueBuilder catalog={catalog} enabled={enabled} onChange={(next) => replace(index, key, next)} schema={{}} value={item} />
          <Button
            aria-label={t("studio.removeField", { key })}
            onClick={() =>
              emit(
                Object.fromEntries(
                  entries.filter((_, current) => current !== index),
                ),
              )
            }
            size="icon"
            variant="ghost"
          >
            <Trash2 className="size-3.5" />
          </Button>
        </div>
      ))}
      <Button
        onClick={() =>
          emit({
            ...Object.fromEntries(entries),
            [`field${entries.length + 1}`]: { kind: "literal", value: "" },
          })
        }
        size="sm"
        variant="secondary"
      >
        <Plus className="size-3.5" />
        {t("studio.addField")}
      </Button>
    </div>
  );
}

function MapperControl({
  value,
  catalog,
  enabled,
  onChange,
  schema,
}: {
  value: unknown;
  catalog?: ReferenceCatalog;
  enabled?: boolean;
  onChange: (value: Record<string, unknown>) => void;
  schema: JsonSchemaProperty;
}) {
  const { t } = useTranslation();
  const binding = asInputBinding(value);
  const fields = binding.kind === "object" ? binding.fields : {};
  const entries = Object.entries(fields);
  const emit = (next: Record<string, unknown>) => onChange({ kind: "object", fields: next } as unknown as Record<string, unknown>);
  const replace = (index: number, key: string, next: unknown) =>
    emit(
      Object.fromEntries(
        entries.map(([currentKey, currentValue], current) =>
          current === index ? [key, next] : [currentKey, currentValue],
        ),
      ),
    );
  const shape = mapperSchemaShape(schema);
  const dynamicValueSchema = schema["x-agentx-binding"]
    ? { "x-agentx-binding": schema["x-agentx-binding"] }
    : {};
  if (shape) {
    const current = Object.fromEntries(entries);
    const unknown = entries.filter(([name]) => !shape.properties[name]);
    const set = (name: string, next: unknown) => emit({ ...current, [name]: next });
    return (
      <div className="space-y-2" data-testid="mapper-control">
        {Object.entries(shape.properties).map(([name, property]) => (
          <div className="grid grid-cols-[120px_minmax(0,1fr)] items-start gap-2" data-field-path={`inputs.${name}`} key={name}>
            <span className="pt-2 text-xs text-muted-foreground"><RequiredLabel required={shape.required.has(name)}>{property.title ?? name}</RequiredLabel></span>
            <AnyJsonValueBuilder catalog={catalog} enabled={enabled} onChange={(next) => set(name, next)} schema={shape.binding ? { ...property, "x-agentx-binding": shape.binding } : property} value={current[name]} />
          </div>
        ))}
        {unknown.map(([name, item]) => (
          <div className="grid grid-cols-[120px_minmax(0,1fr)_32px] gap-2 border-l-2 border-danger pl-2" key={name}>
            <span className="pt-2 text-xs text-danger">{name}</span>
            <AnyJsonValueBuilder catalog={catalog} enabled={enabled} onChange={(next) => set(name, next)} schema={{}} value={item} />
            <Button aria-label={t("studio.removeField", { key: name })} onClick={() => emit(Object.fromEntries(entries.filter(([key]) => key !== name)))} size="icon" variant="ghost"><Trash2 className="size-3.5" /></Button>
          </div>
        ))}
      </div>
    );
  }
  return (
    <div className="space-y-2" data-testid="mapper-control">
      {entries.map(([key, item], index) => (
        <div
          className="grid grid-cols-[minmax(0,1fr)_minmax(0,1.35fr)_32px] gap-2"
          key={`${key}-${index}`}
        >
          <Input
            aria-label={t("studio.key")}
            onChange={(event) => replace(index, event.target.value, item)}
            value={key}
          />
          <AnyJsonValueBuilder catalog={catalog} enabled={enabled} onChange={(next) => replace(index, key, next)} schema={dynamicValueSchema} value={item} />
          <Button
            aria-label={t("studio.removeField", { key })}
            onClick={() =>
              emit(
                Object.fromEntries(
                  entries.filter((_, current) => current !== index),
                ),
              )
            }
            size="icon"
            variant="ghost"
          >
            <Trash2 className="size-3.5" />
          </Button>
        </div>
      ))}
      <Button
        onClick={() =>
          emit({
            ...Object.fromEntries(entries),
            [`field${entries.length + 1}`]: { kind: "literal", value: "" },
          })
        }
        size="sm"
        variant="secondary"
      >
        <Plus className="size-3.5" />
        {t("studio.addField")}
      </Button>
    </div>
  );
}

type ConditionRow = { condition?: unknown; label?: string };
type ConditionGroup = { id?: string; name?: string; conditions?: ConditionRow[]; logicalOp?: string };

function ConditionBuilderControl({
  value,
  catalog,
  onChange,
}: {
  value: unknown;
  catalog?: ReferenceCatalog;
  onChange: (value: unknown) => void;
}) {
  const { t } = useTranslation();
  const [renaming, setRenaming] = useState<string>();
  const logicalOpOptions = [
    { value: "and", label: t("studio.conditionBuilder.and") },
    { value: "or", label: t("studio.conditionBuilder.or") },
  ];
  const renderRows = (
    rows: ConditionRow[],
    updateRows: (rows: ConditionRow[]) => void,
    key: string,
    logicalOp: string,
    updateLogicalOp: (value: string) => void,
  ) => (
    <div className="space-y-1.5">
      {rows.map((row, index) => <div key={`${key}-${index}`}>
        {index > 0 && <div className="my-1 flex items-center gap-2"><span className="h-px flex-1 bg-border" /><Select aria-label={t("studio.conditionBuilder.logicalOp")} className="h-7 w-[76px]" onValueChange={updateLogicalOp} options={logicalOpOptions} value={logicalOp} /><span className="h-px flex-1 bg-border" /></div>}
        <div className="flex items-start gap-2">
          <div className="min-w-0 flex-1"><ConditionRowEditor catalog={catalog} onChange={(condition) => updateRows(rows.map((current, currentIndex) => currentIndex === index ? { ...current, condition } : current))} value={row.condition} /></div>
          <Button aria-label={t("studio.conditionBuilder.removeCondition")} onClick={() => updateRows(rows.filter((_, currentIndex) => currentIndex !== index))} size="icon" variant="ghost"><Trash2 className="size-3.5" /></Button>
        </div>
      </div>)}
      <Button aria-label={t("studio.conditionBuilder.addCondition")} onClick={() => updateRows([...rows, { condition: defaultComparisonCondition() }])} size="sm" variant="ghost"><Plus className="size-3.5" />{t("studio.conditionBuilder.addCondition")}</Button>
    </div>
  );
  if (Array.isArray(value)) {
    const branches = value as ConditionGroup[];
    const updateBranch = (index: number, patch: Partial<ConditionGroup>) => onChange(branches.map((branch, currentIndex) => currentIndex === index ? { ...branch, ...patch } : branch));
    return <div className="space-y-2" data-testid="condition-builder">
      {branches.map((branch, index) => {
        const rows = Array.isArray(branch.conditions) ? branch.conditions : [];
        const branchId = branch.id ?? String(index);
        return <div className="rounded-md border border-border/70 p-2" data-testid={`condition-branch-${index}`} key={branchId}>
          <div className="mb-2 flex h-8 items-center gap-2">
            <span className="shrink-0 rounded bg-muted px-1.5 py-0.5 text-[10px] font-semibold text-muted-foreground">{index === 0 ? t("studio.conditionBuilder.if") : `${t("studio.conditionBuilder.elseIf")} ${index}`}</span>
            {renaming === branchId
              ? <Input autoFocus aria-label={t("studio.inspector.name")} className="h-8 min-w-0 flex-1" onBlur={() => setRenaming(undefined)} onChange={(event) => updateBranch(index, { name: event.target.value })} value={branch.name ?? ""} />
              : <span className="min-w-0 flex-1 truncate text-[11px] text-muted-foreground">{branch.name || t("studio.conditionBuilder.defaultBranchName", { index: index + 1 })}</span>}
            <Button aria-label={t("studio.conditionBuilder.renameBranch")} onClick={() => setRenaming(branchId)} size="icon" variant="ghost"><Pencil className="size-3.5" /></Button>
            {branches.length > 1 && <Button aria-label={t("studio.conditionBuilder.removeBranch")} onClick={() => onChange(branches.filter((_, currentIndex) => currentIndex !== index))} size="icon" variant="ghost"><Trash2 className="size-3.5" /></Button>}
          </div>
          {renderRows(rows, (conditions) => updateBranch(index, { conditions }), branchId, branch.logicalOp ?? "and", (logicalOp) => updateBranch(index, { logicalOp }))}
        </div>;
      })}
      <Button aria-label={t("studio.conditionBuilder.addBranch")} onClick={() => onChange([...branches, { id: nextConditionId(branches), name: "", conditions: [{ condition: defaultComparisonCondition() }], logicalOp: "and" }])} size="sm" variant="secondary"><Plus className="size-3.5" />{t("studio.conditionBuilder.addBranch")}</Button>
    </div>;
  }
  const group = value && typeof value === "object" ? value as ConditionGroup : {};
  const rows = Array.isArray(group.conditions) ? group.conditions : [];
  return <div className="space-y-2 rounded-md border border-border/70 p-2" data-testid="condition-builder">
    <span className="text-[10px] font-medium text-muted-foreground">{t("studio.conditionBuilder.conditions")}</span>
    {renderRows(rows, (conditions) => onChange({ ...group, conditions }), "conditions", group.logicalOp ?? "and", (logicalOp) => onChange({ ...group, logicalOp }))}
  </div>;
}

function nextConditionId(branches: ConditionGroup[]) {
  const taken = new Set(branches.map((branch) => branch.id));
  let id = `case_${crypto.randomUUID()}`;
  while (taken.has(id)) id = `case_${crypto.randomUUID()}`;
  return id;
}

function mapperSchemaShape(schema: JsonSchemaProperty) {
  const candidate = (schema as JsonSchemaProperty & { allOf?: JsonSchemaProperty[] }).allOf?.find((item) => item.properties) ?? schema;
  if (!candidate.properties || !Object.keys(candidate.properties).length) return undefined;
  return {
    properties: candidate.properties,
    required: new Set(candidate.required ?? []),
    binding: schema["x-agentx-binding"]?.recursive ? schema["x-agentx-binding"] : undefined,
  };
}

type EditorButton = { id?: string; label?: string };

function ButtonsEditorControl({
  value,
  onChange,
}: {
  value: unknown;
  onChange: (value: unknown) => void;
}) {
  const { t } = useTranslation();
  const record = value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};
  const buttons = Array.isArray(value)
    ? (value as EditorButton[])
    : Array.isArray(record.buttons)
      ? (record.buttons as EditorButton[])
      : [];
  const update = (next: EditorButton[]) =>
    onChange(Array.isArray(value) ? next : { ...record, buttons: next });
  const replace = (index: number, patch: Partial<EditorButton>) =>
    update(buttons.map((button, currentIndex) => currentIndex === index ? { ...button, ...patch } : button));
  return (
    <div className="space-y-2" data-testid="buttons-editor">
      {buttons.map((button, index) => (
        <div className="grid grid-cols-[minmax(0,1fr)_32px] gap-2" key={button.id ?? index}>
          <Input
            aria-label={t("studio.buttonsEditor.buttonLabel")}
            onChange={(event) => replace(index, { label: event.target.value })}
            value={button.label ?? ""}
          />
          <Button
            aria-label={t("studio.buttonsEditor.removeButton")}
            onClick={() => update(buttons.filter((_, currentIndex) => currentIndex !== index))}
            size="icon"
            variant="ghost"
          >
            <Trash2 className="size-3.5" />
          </Button>
        </div>
      ))}
      <Button aria-label={t("studio.buttonsEditor.addButton")} onClick={() => update([...buttons, { id: nextButtonId(buttons), label: "" }])} size="sm" variant="secondary">
        <Plus className="size-3.5" />
        {t("studio.buttonsEditor.addButton")}
      </Button>
    </div>
  );
}

function nextButtonId(buttons: EditorButton[]) {
  const taken = new Set(buttons.map((button) => button.id));
  let id = `decision_${crypto.randomUUID()}`;
  while (taken.has(id)) id = `decision_${crypto.randomUUID()}`;
  return id;
}

function ConditionRowEditor({
  value,
  catalog,
  onChange,
}: {
  value: unknown;
  catalog?: ReferenceCatalog;
  onChange: (value: ConditionSpec) => void;
}) {
  const { t } = useTranslation();
  const condition = asConditionSpec(value);
  const leftSchema = inputBindingSchema(condition.left, catalog);
  const operators = operatorsForSchema(leftSchema);
  const unary = condition.operator === "is_empty" || condition.operator === "is_not_empty";
  const update = (patch: Partial<ConditionSpec>) => onChange({ ...condition, ...patch });
  return (
    <div className={`grid gap-2 ${unary ? "grid-cols-[minmax(0,1fr)_112px]" : "grid-cols-[minmax(0,1fr)_92px_minmax(0,1fr)]"}`} data-testid="condition-comparison-row">
      <SmartInput allowedNamespaces={["inputs", "outputs", "contexts", "execution", "item", "loop"]} catalog={catalog} onChange={(left) => update({ left })} value={condition.left} />
      <Select
        aria-label={t("studio.conditionBuilder.operator")}
        className="h-9"
        onValueChange={(operator) => update({ operator: operator as ConditionOperator })}
        options={operators.map((operator) => ({ value: operator, label: conditionOperatorLabel(operator) }))}
        value={operators.includes(condition.operator) ? condition.operator : "eq"}
      />
      {!unary && <SmartInput allowedNamespaces={["inputs", "outputs", "contexts", "execution", "item", "loop"]} catalog={catalog} expectedSchema={leftSchema} onChange={(right) => update({ right })} value={condition.right ?? { kind: "literal", value: "" }} />}
    </div>
  );
}

function defaultComparisonCondition(): ConditionSpec {
  return { left: { kind: "literal", value: "" }, operator: "eq", right: { kind: "literal", value: "" } };
}

function conditionOperatorLabel(operator: ConditionOperator) {
  return ({ eq: "=", ne: "≠", gt: ">", gte: "≥", lt: "<", lte: "≤", in: "IN", contains: "包含", not_contains: "不包含", starts_with: "开头为", ends_with: "结尾为", matches: "匹配", is_empty: "为空", is_not_empty: "不为空" })[operator];
}

function asConditionSpec(value: unknown): ConditionSpec {
  if (value && typeof value === "object" && "left" in value && "operator" in value) return value as ConditionSpec;
  return defaultComparisonCondition();
}

function operatorsForSchema(schema?: JsonSchemaProperty): ConditionOperator[] {
  const type = primarySchemaType(schema ?? {});
  if (type === "string") return ["eq", "ne", "contains", "not_contains", "starts_with", "ends_with", "matches", "is_empty", "is_not_empty"];
  if (type === "number" || type === "integer") return ["eq", "ne", "gt", "gte", "lt", "lte", "is_empty", "is_not_empty"];
  if (type === "array") return ["contains", "not_contains", "is_empty", "is_not_empty"];
  if (type === "object") return ["is_empty", "is_not_empty"];
  if (type === "boolean") return ["eq", "ne"];
  return ["eq", "ne", "is_empty", "is_not_empty"];
}

function inputBindingSchema(binding: InputBinding, catalog?: ReferenceCatalog): JsonSchemaProperty | undefined {
  switch (binding.kind) {
    case "reference": return referenceSchema(catalog, binding.selector);
    case "template": return { type: "string" };
    case "array": return { type: "array" };
    case "object": return { type: "object" };
    case "literal": {
      if (binding.value === null) return undefined;
      if (Array.isArray(binding.value)) return { type: "array" };
      if (typeof binding.value === "number") return { type: Number.isInteger(binding.value) ? "integer" : "number" };
      if (typeof binding.value === "boolean") return { type: "boolean" };
      if (typeof binding.value === "object") return { type: "object" };
      return { type: "string" };
    }
  }
}

function referenceSchema(catalog: ReferenceCatalog | undefined, selector: ValueSelector): JsonSchemaProperty | undefined {
  const visit = (entries: import("../model/types").ReferenceEntry[]): JsonSchemaProperty | undefined => {
    for (const entry of entries) {
      if (entry.selector && selectorsEqual(entry.selector, selector)) return entry.schema;
      const child = visit(entry.children);
      if (child) return child;
    }
    return undefined;
  };
  for (const entries of Object.values(catalog ?? {})) {
    const found = visit(entries ?? []);
    if (found) return found;
  }
  return undefined;
}

function UnsupportedControl({
  label,
  control,
}: {
  label: string;
  control?: string;
}) {
  const { t } = useTranslation();
  return (
    <div className="border border-danger/40 bg-danger/5 p-3 text-xs text-danger">
      <div className="flex items-center gap-2 font-medium">
        <AlertTriangle className="size-3.5" />
        {label}
      </div>
      <p className="mt-1 text-[10px]">
        {t("studio.unsupported", { control: control ?? "missing" })}
      </p>
    </div>
  );
}
function Field({
  children,
  label,
  unit,
  required,
  error,
  testId,
}: {
  children: React.ReactNode;
  label: string;
  unit?: string;
  required?: boolean;
  error?: string;
  testId?: string;
}) {
  return (
    <div className="block text-xs" data-testid={testId}>
      <span className="mb-1.5 flex items-center justify-between gap-2 text-muted-foreground">
        <span><RequiredLabel required={required}>{label}</RequiredLabel></span>
        {unit && <span className="rounded bg-muted px-1.5 py-0.5 text-[10px] font-medium text-muted-foreground">{unit}</span>}
      </span>
      {children}
      {error && (
        <span className="mt-1 block text-[10px] text-danger">{error}</span>
      )}
    </div>
  );
}
const humanize = (value: string) =>
  value
    .replace(/([a-z])([A-Z])/g, "$1 $2")
    .replaceAll("_", " ")
    .replace(/^./, (letter) => letter.toUpperCase());
const codeLanguage = (runner: unknown) =>
  runner === "javascript"
    ? "javascript"
    : runner === "shell"
      ? "shell"
      : "python";
const inferredUnit = (name: string) => {
  if (name.endsWith("Ms")) return "milliseconds";
  if (name.includes("Tokens")) return "tokens";
  if (name.includes("Calls") || name.includes("Iterations")) return "calls";
  if (name === "maxItems" || name === "batchSize") return "items";
  return undefined;
};
const selectOption = (item: unknown, localizedLabel?: string) =>
  item !== null &&
  typeof item === "object" &&
  "value" in item &&
  "label" in item
    ? { value: String(item.value), label: localizedLabel ?? String(item.label) }
    : { value: String(item), label: localizedLabel ?? String(item) };
const schemaHasType = (schema: JsonSchemaProperty | undefined, type: string) => schema?.type === type || Array.isArray(schema?.type) && schema.type.includes(type);
const primarySchemaType = (schema: JsonSchemaProperty) => Array.isArray(schema.type) ? schema.type.find((type) => type !== "null") ?? schema.type[0] : schema.type;
const allowedNamespaces = (schema: JsonSchemaProperty): ReferenceNamespace[] =>
  (schema["x-agentx-binding"]?.allowedNamespaces ?? ["inputs", "outputs", "contexts"]).filter(
    (namespace): namespace is ReferenceNamespace =>
      namespace === "inputs" ||
      namespace === "outputs" ||
      namespace === "contexts" ||
      namespace === "item" ||
      namespace === "execution" ||
      namespace === "loop",
  );
export const SUPPORTED_CONTROLS = new Set([
  "text",
  "textarea",
  "number",
  "boolean",
  "select",
  "provider_options",
  "collection",
  "fixed_collection",
  "mapper",
  "reference",
  "value",
  "template",
  "structured",
  "prompt",
  "json",
  "code",
  "condition_builder",
  "buttons_editor",
  "schema_editor",
  "json5_example",
  "network_policy",
  "kv_builder",
  "sort_builder",
  "api_key_placement",
]);
