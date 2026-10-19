import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";

import { Button } from "../../../shared/ui/button";
import { localizedValue } from "../../../shared/lib/localized-value";
import { Dialog, DialogContent } from "../../../shared/ui/dialog";
import { Input } from "../../../shared/ui/input";
import { Select } from "../../../shared/ui/select";
import { Textarea } from "../../../shared/ui/textarea";
import { Tooltip } from "../../../shared/ui/tooltip";
import { SchemaForm, type JsonSchema } from "../../../shared/components/schema-form";
import { StructuredJsonControl } from "../forms/parameter-field";
import type {
  ContextDefinition,
  JsonSchemaProperty,
  WorkflowStart,
} from "../model/types";

import { WaitInputsEditor, type WaitInputNode, type WaitInputs } from "../../../shared/components/wait-inputs-editor";

type Values = Record<string, unknown>;
type FieldErrors = Record<string, string>;

export function hasRunParameters(start: WorkflowStart, waitNodes: readonly WaitInputNode[] = []) {
  const schema = asSchema(start.inputs);
  return (
    waitNodes.length > 0 ||
    Object.keys(schema.properties ?? {}).length > 0 ||
    Object.values(start.contexts).some((context) => context.clientWritable)
  );
}

export function RunParametersDialog({
  open,
  running,
  start,
  waitNodes = [],
  onClose,
  onRun,
}: {
  open: boolean;
  running: boolean;
  start: WorkflowStart;
  waitNodes?: readonly WaitInputNode[];
  onClose: () => void;
  onRun: (input: Values, context: Values, waitInputs: WaitInputs) => void;
}) {
  const { t } = useTranslation();
  const inputSchema = useMemo(() => asSchema(start.inputs), [start.inputs]);
  const inputFields = useMemo(
    () => Object.entries(inputSchema.properties ?? {}),
    [inputSchema],
  );
  const contextFields = useMemo(
    () =>
      Object.entries(start.contexts).filter(
        ([, context]) => context.clientWritable,
      ),
    [start.contexts],
  );
  const [repliesValid, setRepliesValid] = useState(true);
  const [waitInputs, setWaitInputs] = useState<WaitInputs>({});
  const [input, setInput] = useState<Values>({});
  const [context, setContext] = useState<Values>({});
  const [errors, setErrors] = useState<FieldErrors>({});

  useEffect(() => {
    if (!open) return;
    setInput(schemaDefaults(inputSchema));
    setContext(contextDefaults(contextFields));
    setErrors({});
    setWaitInputs({});
  }, [open, inputSchema, contextFields]);

  const submit = () => {
    const nextErrors: FieldErrors = {};
    const required = new Set(inputSchema.required ?? []);
    for (const [name, schema] of inputFields) {
      const message = validateValue(schema, input[name], required.has(name), t);
      if (message) nextErrors[`input.${name}`] = message;
    }
    for (const [name, definition] of contextFields) {
      const schema = asSchema(definition.schema);
      const message = validateValue(schema, context[name], false, t);
      if (message) nextErrors[`context.${name}`] = message;
      if (
        definition.maxSize &&
        context[name] !== undefined &&
        new TextEncoder().encode(JSON.stringify(context[name])).length > definition.maxSize
      ) {
        nextErrors[`context.${name}`] = t("studio.runParameters.maxSize", {
          size: definition.maxSize,
        });
      }
    }
    setErrors(nextErrors);
    if (repliesValid && Object.keys(nextErrors).length === 0) onRun(input, context, waitInputs);
  };

  return (
    <Dialog onOpenChange={(value) => !value && onClose()} open={open}>
      <DialogContent
        className="w-[min(620px,calc(100vw-32px))]"
        description={t("studio.runParameters.description")}
        title={t("studio.runParameters.title")}
      >
        <div className="border-b border-border px-5 py-4">
          <h2 className="text-sm font-semibold">
            {t("studio.runParameters.title")}
          </h2>
        </div>
        <div className="max-h-[65vh] space-y-6 overflow-y-auto p-5">
          <WaitInputsEditor onValidityChange={setRepliesValid} nodes={waitNodes} value={waitInputs} onChange={setWaitInputs} disabled={running} />
          {inputFields.length > 0 && (
            <FieldSection title={t("studio.runParameters.inputs")}>
              <div className="sm:col-span-2"><SchemaForm errors={Object.fromEntries(Object.entries(errors).filter(([name]) => name.startsWith("input.")).map(([name, message]) => [name.slice(6), message]))} onChange={setInput} schema={inputSchema as JsonSchema} values={input} /></div>
            </FieldSection>
          )}
          {contextFields.length > 0 && (
            <FieldSection title={t("studio.runParameters.contexts")}>
              {contextFields.map(([name, definition]) => (
                <SchemaField
                  context={definition}
                  error={errors[`context.${name}`]}
                  key={name}
                  name={name}
                  onChange={(value) =>
                    setContext((current) => ({ ...current, [name]: value }))
                  }
                  schema={asSchema(definition.schema)}
                  value={context[name]}
                />
              ))}
            </FieldSection>
          )}
        </div>
        <div className="flex justify-end gap-2 border-t border-border px-5 py-4">
          <Button onClick={onClose} variant="ghost">
            {t("common.cancel")}
          </Button>
          <Button disabled={running || !repliesValid} onClick={submit}>
            {t("studio.runParameters.run")}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}

function FieldSection({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    <section>
      <h3 className="mb-3 text-xs font-semibold text-foreground">{title}</h3>
      <div className="grid gap-4 sm:grid-cols-2">{children}</div>
    </section>
  );
}

function SchemaField({
  name,
  schema,
  value,
  required = false,
  error,
  context,
  onChange,
}: {
  name: string;
  schema: JsonSchemaProperty;
  value: unknown;
  required?: boolean;
  error?: string;
  context?: ContextDefinition;
  onChange: (value: unknown) => void;
}) {
  const { t } = useTranslation();
  const label = schema.title || context?.title || humanize(name);
  const description = schema.description || context?.description;
  const type = schema.type ?? "string";
  const wide = type === "object" || type === "array" || schema.multiline;
  const fieldId = `run-parameter-${context ? "context" : "input"}-${name}`;

  return (
    <div className={wide ? "sm:col-span-2" : undefined}>
      <label className="mb-1.5 block text-xs font-medium" htmlFor={fieldId}>
        {description ? (
          <Tooltip content={description}>
            <span
              className="cursor-help border-b border-dotted border-muted-foreground/60 outline-none focus-visible:ring-2 focus-visible:ring-primary/30"
              tabIndex={0}
            >
              {label}
            </span>
          </Tooltip>
        ) : (
          label
        )}
        {required && <span className="ml-1 text-danger">*</span>}
        {context && (
          <span className="ml-2 text-[10px] font-normal text-muted-foreground">
            {localizedValue(t, "studio.contextScopes", context.scope)}
          </span>
        )}
      </label>
      <SchemaControl
        context={context}
        id={fieldId}
        label={label}
        onChange={onChange}
        schema={schema}
        value={value}
      />
      {error && <p className="mt-1 text-[10px] text-danger">{error}</p>}
    </div>
  );
}

function SchemaControl({
  id,
  label,
  schema,
  value,
  context,
  onChange,
}: {
  id: string;
  label: string;
  schema: JsonSchemaProperty;
  value: unknown;
  context?: ContextDefinition;
  onChange: (value: unknown) => void;
}) {
  const { t } = useTranslation();
  if (schema.enum?.length) {
    return (
      <Select
        aria-label={label}
        className="w-full"
        onValueChange={(selected) =>
          onChange(schema.enum?.find((item) => String(item) === selected))
        }
        options={schema.enum.map((item) => ({
          value: String(item),
          label: String(item),
        }))}
        placeholder={t("studio.runParameters.selectPlaceholder")}
        value={value === undefined || value === null ? "" : String(value)}
      />
    );
  }
  if (schema.type === "boolean") {
    return (
      <label
        className="flex h-9 items-center gap-2 rounded-lg border border-border bg-surface px-3 text-xs"
        htmlFor={id}
      >
        <input
          aria-label={label}
          checked={Boolean(value)}
          className="size-4 accent-primary"
          id={id}
          onChange={(event) => onChange(event.target.checked)}
          type="checkbox"
        />
        {value
          ? t("studio.runParameters.enabled")
          : t("studio.runParameters.disabled")}
      </label>
    );
  }
  if (schema.type === "number" || schema.type === "integer") {
    return (
      <Input
        aria-label={label}
        id={id}
        max={schema.maximum}
        min={schema.minimum}
        onChange={(event) =>
          onChange(event.target.value ? Number(event.target.value) : undefined)
        }
        step={schema.type === "integer" ? 1 : "any"}
        type="number"
        value={typeof value === "number" ? value : ""}
      />
    );
  }
  if (schema.type === "object" || schema.type === "array") {
    return (
      <StructuredJsonControl
        fallback={schema.type === "array" ? [] : {}}
        onChange={onChange}
        schema={schema}
        value={value}
      />
    );
  }
  if (schema.multiline) {
    return (
      <Textarea
        aria-label={label}
        className="min-h-28"
        id={id}
        maxLength={schema.maxLength}
        minLength={schema.minLength}
        onChange={(event) => onChange(event.target.value)}
        value={String(value ?? "")}
      />
    );
  }
  return (
    <Input
      aria-label={label}
      id={id}
      maxLength={schema.maxLength}
      minLength={schema.minLength}
      onChange={(event) => onChange(event.target.value)}
      type={inputType(schema.format, context?.sensitive)}
      value={String(value ?? "")}
    />
  );
}

function asSchema(value: unknown): JsonSchemaProperty {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as JsonSchemaProperty)
    : { type: "object", properties: {} };
}

function schemaDefaults(schema: JsonSchemaProperty): Values {
  return Object.fromEntries(
    Object.entries(schema.properties ?? {}).flatMap(([name, property]) => {
      const value = defaultValue(property);
      return value === undefined ? [] : [[name, value]];
    }),
  );
}

function contextDefaults(fields: Array<[string, ContextDefinition]>): Values {
  return Object.fromEntries(
    fields.flatMap(([name, definition]) =>
      definition.default === undefined || definition.default === null
        ? []
        : [[name, definition.default]],
    ),
  );
}

function defaultValue(schema: JsonSchemaProperty): unknown {
  if (schema.default !== undefined) return schema.default;
  if (schema.type === "object" && schema.properties) {
    const nested = schemaDefaults(schema);
    return Object.keys(nested).length ? nested : undefined;
  }
  return undefined;
}

function validateValue(
  schema: JsonSchemaProperty,
  value: unknown,
  required: boolean,
  t: (key: string, options?: Record<string, unknown>) => string,
) {
  if (value === undefined || value === null || value === "") {
    return required ? t("studio.runParameters.required") : undefined;
  }
  if (schema.enum && !schema.enum.some((item) => item === value))
    return t("studio.runParameters.invalidOption");
  if (typeof value === "string") {
    if (schema.minLength !== undefined && value.length < schema.minLength)
      return t("studio.runParameters.minLength", { length: schema.minLength });
    if (schema.maxLength !== undefined && value.length > schema.maxLength)
      return t("studio.runParameters.maxLength", { length: schema.maxLength });
  }
  if (typeof value === "number") {
    if (schema.type === "integer" && !Number.isInteger(value))
      return t("studio.runParameters.integer");
    if (schema.minimum !== undefined && value < schema.minimum)
      return t("studio.runParameters.minimum", { value: schema.minimum });
    if (schema.maximum !== undefined && value > schema.maximum)
      return t("studio.runParameters.maximum", { value: schema.maximum });
  }
  return undefined;
}

function inputType(format?: string, sensitive?: boolean) {
  if (sensitive || format === "password") return "password";
  if (format === "email") return "email";
  if (format === "uri" || format === "url") return "url";
  if (format === "date") return "date";
  if (format === "time") return "time";
  if (format === "date-time") return "datetime-local";
  return "text";
}

function humanize(value: string) {
  return value
    .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
    .replace(/[_-]+/g, " ")
    .replace(/^./, (character) => character.toUpperCase());
}
