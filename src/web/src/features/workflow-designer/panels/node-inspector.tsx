import { useMutation, useQuery } from "@tanstack/react-query";
import { MoreHorizontal, Play, Power, Trash2, X } from "lucide-react";
import { useEffect, useMemo, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";

import { apiRequest } from "../../../shared/api/client";
import { Button } from "../../../shared/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "../../../shared/ui/dropdown-menu";
import { Input } from "../../../shared/ui/input";
import { ReadOnlyForm } from "../forms/read-only-form";
import { Select } from "../../../shared/ui/select";
import { Dialog, DialogContent } from "../../../shared/ui/dialog";
import {
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from "../../../shared/ui/tabs";
import { useProviderOptions } from "../api/use-provider-options";
import { deleteOverlay, saveOverlay } from "../api/studio-api";
import type {
  NodeExecution,
} from "../../../shared/api/types";
import { TraceNodeDetails } from "../../traces/trace-node-card";
import { useExecutionArtifactDownload } from "../../traces/use-execution-artifact-download";
import type {
  ActionNodeData,
  JsonSchemaProperty,
  NodeManifest,
  ReferenceCatalog,
  ReferenceEntry,
  ValueSelector,
  ResourceOption,
  ResourceRequestContext,
  ResourceType,
  UiField,
} from "../model/types";
import {
  localizeManifest,
  localizedNodeLabel,
} from "../model/manifest-localization";
import { SUPPORTED_CONTROLS } from "../forms/parameter-field";
import { isReferenceKey } from "../utils/definition-validation";
import { InspectorShell } from "./inspector-shell";
import { nodeGroupColor } from "../nodes/node-appearance";
import { NodeIcon } from "../nodes/node-icon";
import { PluginResult } from "../nodes/plugin-result";
import { Badge } from "../../../shared/ui/badge";
import {
  PanelField,
} from "./inspectors/panel-shell";
import { resolveNodePanel } from "./node-panel-registry";

export function NodeInspector({
  data,
  nodeId,
  manifest,
  pluginVersions = [],
  pluginVersionContext,
  resources,
  workflowId,
  draftId,
  executionId,
  referenceCatalog,
  parameterCatalogs,
  open = true,
  onClose,
  onRun,
  onChange,
  onDelete,
  onOverlayChange,
  fieldErrors = {},
  onValidityChange,
  onResourceAuthorize,
  onResourceRequest,
  readOnly = false,
}: {
  data?: ActionNodeData;
  nodeId?: string;
  manifest?: NodeManifest;
  pluginVersions?: NodeManifest[];
  pluginVersionContext?: { affectedNodes: number; connectedInputs: string[]; connectedOutputs: string[]; dependentReferences: number };
  resources: Partial<Record<ResourceType, ResourceOption[]>>;
  workflowId?: string;
  draftId?: string;
  executionId?: string;
  referenceCatalog?: ReferenceCatalog;
  parameterCatalogs?: Record<string, ReferenceCatalog | undefined>;
  open?: boolean;
  onClose?: () => void;
  onRun?: () => void;
  onChange: (data: Partial<ActionNodeData>) => void;
  onDelete: () => void;
  onOverlayChange?: (nodeId: string, overlayId?: string) => void;
  fieldErrors?: Record<string, string>;
  onValidityChange?: (valid: boolean) => void;
  onResourceAuthorize?: (option: ResourceOption) => Promise<void>;
  onResourceRequest?: (option: ResourceOption, context?: ResourceRequestContext) => Promise<void>;
  readOnly?: boolean;
}) {
  const { t, i18n } = useTranslation();
  const downloadArtifact = useExecutionArtifactDownload(executionId);
  const [tab, setTab] = useState("parameters");
  const providerOptions = useProviderOptions(manifest, data?.parameters, data?.resourceReferences);
  const resolvedManifest = data?.nodeType === "sub_workflow"
    ? providerOptions.workflowVersionId?.find((option) => option.value === data.parameters.workflowVersionId)?.manifest ?? manifest
    : manifest;
  const unsupported = parameterEntries(resolvedManifest).some(
    ([name]) =>
      !SUPPORTED_CONTROLS.has(uiFields(resolvedManifest)[name]?.control ?? ""),
  );
  const dataKey = data?.nodeType;
  const localized = resolvedManifest
    ? localizeManifest(resolvedManifest, i18n.language)
    : undefined;
  const title =
    data && resolvedManifest
      ? localizedNodeLabel(resolvedManifest, data.label, i18n.language)
      : (data?.label ?? "");
  const currentNodeCatalog = useMemo(
    () =>
      data?.editorKind === "action" && resolvedManifest && referenceCatalog
        ? addCurrentNodeReferences(referenceCatalog, data, resolvedManifest, t("studio.references.currentRawOutput"), nodeId)
        : referenceCatalog,
    [data, resolvedManifest, nodeId, referenceCatalog, t],
  );
  const parameterCatalog = useMemo(
    () => referenceCatalog
      ? addCurrentItemReference(referenceCatalog, t("studio.references.currentInput"))
      : referenceCatalog,
    [referenceCatalog, t],
  );
  const translate = (key: string, fallback?: string) =>
    fallback ? t(key, { defaultValue: fallback }) : t(key);
  const nodeRuns = useQuery({
    queryKey: ["studio-node-inspector", executionId, nodeId],
    queryFn: () =>
      apiRequest<{ items: NodeExecution[] }>(
        `/executions/${executionId}/nodes`,
      ),
    enabled: Boolean(executionId && nodeId),
    refetchInterval: executionId ? 2_000 : false,
  });
  const selectedRuns = useMemo(
    () => nodeRuns.data?.items.filter((node) => node.nodeId === nodeId) ?? [],
    [nodeId, nodeRuns.data?.items],
  );
  const selectedRun = selectedRuns.at(-1);
  const [overlayText, setOverlayText] = useState("{}");
  const overlay = useMutation({
    mutationFn: (kind: string) =>
      saveOverlay(workflowId!, draftId!, nodeId!, kind, JSON.parse(overlayText)),
    onSuccess: (value) => nodeId && onOverlayChange?.(nodeId, value.id),
  });
  const removeOverlay = useMutation({
    mutationFn: () => deleteOverlay(workflowId!, draftId!, nodeId!),
    onSuccess: () => nodeId && onOverlayChange?.(nodeId),
  });

  useEffect(
    () => onValidityChange?.(!unsupported),
    [onValidityChange, unsupported],
  );
  useEffect(() => {
    setOverlayText("{}");
  }, [nodeId]);
  useEffect(() => {
    if (selectedRun?.output != null)
      setOverlayText(JSON.stringify(selectedRun.output, null, 2));
  }, [selectedRun?.output]);
  useEffect(() => {
    if (data) setTab("parameters");
  }, [data, dataKey]);

  if (!open || !data) return null;
  return (
    <InspectorShell testId="node-details-view">
      <div className="relative z-[101] flex shrink-0 items-center gap-3 border-b border-border bg-surface px-4 py-3">
        <span className="grid size-9 place-items-center rounded-lg" style={{ color: nodeGroupColor(data.nodeType), backgroundColor: `color-mix(in srgb, ${nodeGroupColor(data.nodeType)} 12%, transparent)` }}>
          <NodeIcon
            className="size-5"
            iconKey={resolvedManifest?.iconKey ?? "box"}
          />
        </span>
        <div className="min-w-0 flex-1">
          <strong className="block truncate text-sm font-semibold">
            {title}
          </strong>
          <div className="flex min-w-0 items-center gap-2">
            <span className="block truncate text-[10px] text-muted-foreground">
              {localized?.displayName ?? data.nodeType}
            </span>
            {data.editorKind === "action" && data.disabled && (
              <Badge className="shrink-0 px-1.5 py-0.5 text-[10px]" tone="warning">
                {t("studio.inspector.disabled")}
              </Badge>
            )}
          </div>
        </div>
        {!readOnly && onRun && data.editorKind === "action" && (
          <Button
            aria-label={t("studio.run")}
            onClick={onRun}
            size="icon"
            variant="ghost"
          >
            <Play className="size-4" />
          </Button>
        )}
        {!readOnly && <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button
              aria-label={t("studio.details.more")}
              size="icon"
              variant="ghost"
            >
              <MoreHorizontal className="size-4" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            {onRun && data.editorKind === "action" && (
              <DropdownMenuItem onSelect={onRun}>
                <Play className="size-4" />
                {t("studio.run")}
              </DropdownMenuItem>
            )}
            {data.editorKind === "action" && (
              <DropdownMenuItem onSelect={() => onChange({ disabled: !data.disabled })}>
                <Power className="size-4" />
                {data.disabled
                  ? t("studio.inspector.enable")
                  : t("studio.inspector.disable")}
              </DropdownMenuItem>
            )}
            <DropdownMenuItem className="text-danger" onSelect={onDelete}>
              <Trash2 className="size-4" />
              {t("studio.inspector.delete")}
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>}
        <Button
          aria-label={t("studio.close")}
          onClick={onClose}
          size="icon"
          variant="ghost"
        >
          <X className="size-4" />
        </Button>
      </div>
      <Tabs
        className="flex min-h-0 flex-1 flex-col"
        onValueChange={setTab}
        value={tab}
      >
        <TabsList className="h-11 shrink-0 justify-start gap-4 border-b border-border px-4">
          <TabsTrigger className="text-xs" value="parameters">
            {t("studio.details.parameters")}
          </TabsTrigger>
          <TabsTrigger className="text-xs" value="input">
            {t("studio.details.input")}
          </TabsTrigger>
          <TabsTrigger className="text-xs" value="output">
            {t("studio.details.output")}
          </TabsTrigger>
          <TabsTrigger className="text-xs" value="trace">
            {t("studio.details.trace")}
          </TabsTrigger>
        </TabsList>
        <TabsContent
          className="min-h-0 flex-1 overflow-y-auto p-4"
          value="parameters"
        >
          <ReadOnlyForm readOnly={readOnly} className="min-w-0"><Parameters
            data={data}
            fieldErrors={fieldErrors}
            localized={localized}
            manifest={resolvedManifest}
            pluginVersions={pluginVersions}
            pluginVersionContext={pluginVersionContext}
            onChange={onChange}
            providerOptions={providerOptions}
            referenceCatalog={parameterCatalog}
            parameterCatalogs={parameterCatalogs}
            currentNodeCatalog={currentNodeCatalog}
            resources={resources}
            title={title}
            onResourceAuthorize={onResourceAuthorize}
            onResourceRequest={onResourceRequest}
            readOnly={readOnly}
            sourceNodeId={nodeId}
            t={translate}
            workflowId={workflowId}
          /></ReadOnlyForm>
        </TabsContent>
        <TabsContent className="min-h-0 flex-1 overflow-auto p-4" value="input">
          <JsonValue value={selectedRun?.input} />
        </TabsContent>
        <TabsContent
          className="min-h-0 flex-1 overflow-auto p-4"
          value="output"
        >
          {resolvedManifest?.plugin?.uiSource && selectedRun?.output !== undefined && <PluginResult manifest={resolvedManifest} value={selectedRun.output} />}
          {!readOnly && <div className="mb-3 flex items-center gap-1">
            <Button
              disabled={readOnly || overlay.isPending || data.editorKind !== "action" || selectedRun?.output == null}
              onClick={() => overlay.mutate("pin_data")}
              size="sm"
              variant="secondary"
            >
              {t("studio.runtime.pin")}
            </Button>
            <Button
              disabled={readOnly || overlay.isPending || data.editorKind !== "action"}
              onClick={() => overlay.mutate("mock_output")}
              size="sm"
              variant="ghost"
            >
              {t("studio.runtime.mock")}
            </Button>
            <Button
              aria-label={t("studio.runtime.removeOverlay")}
              disabled={readOnly || removeOverlay.isPending || data.editorKind !== "action"}
              onClick={() => removeOverlay.mutate()}
              size="icon"
              variant="ghost"
            >
              <Trash2 className="size-3.5" />
            </Button>
          </div>}
          <textarea
            aria-label={t("studio.details.output")}
            className="min-h-[260px] w-full resize-y rounded-md border border-border bg-canvas p-3 font-mono text-[11px] leading-5"
            onChange={(event) => setOverlayText(event.target.value)}
            value={overlayText}
            readOnly={readOnly}
          />
        </TabsContent>
        <TabsContent className="min-h-0 flex-1 overflow-auto" value="trace">
          {executionId && selectedRun ? <TraceNodeDetails active={tab === "trace"} executionId={executionId} node={selectedRun} onDownloadArtifact={downloadArtifact} /> : <p className="p-4 text-xs text-muted-foreground">{executionId ? t("trace.noNodeRuns") : t("studio.runtime.noExecution")}</p>}
        </TabsContent>
      </Tabs>
      {!readOnly && <div className="shrink-0 border-t border-border p-4">
        <Button className="w-full" disabled={readOnly} onClick={onDelete} variant="secondary">
          <Trash2 className="size-4" />
          {t("studio.inspector.delete")}
        </Button>
      </div>}
    </InspectorShell>
  );
}

function Parameters({
  data,
  manifest,
  pluginVersions,
  pluginVersionContext,
  localized,
  resources,
  workflowId,
  draftId,
  providerOptions,
  referenceCatalog,
  parameterCatalogs,
  currentNodeCatalog,
  fieldErrors,
  onChange,
  t,
  title,
  onResourceAuthorize,
  onResourceRequest,
  readOnly,
  sourceNodeId,
}: {
  data: ActionNodeData;
  manifest?: NodeManifest;
  pluginVersions: NodeManifest[];
  pluginVersionContext?: { affectedNodes: number; connectedInputs: string[]; connectedOutputs: string[]; dependentReferences: number };
  localized?: ReturnType<typeof localizeManifest>;
  resources: Partial<Record<ResourceType, ResourceOption[]>>;
  workflowId?: string;
  draftId?: string;
  providerOptions: Record<string, ResourceOption[]>;
  referenceCatalog?: ReferenceCatalog;
  parameterCatalogs?: Record<string, ReferenceCatalog | undefined>;
  currentNodeCatalog?: ReferenceCatalog;
  fieldErrors: Record<string, string>;
  onChange: (data: Partial<ActionNodeData>) => void;
  t: (key: string, fallback?: string) => string;
  title: string;
  onResourceAuthorize?: (option: ResourceOption) => Promise<void>;
  onResourceRequest?: (option: ResourceOption, context?: ResourceRequestContext) => Promise<void>;
  readOnly: boolean;
  sourceNodeId?: string;
}) {
  const nameError = !data.label.trim() || data.label.length > 160 ? t("studio.validation.issues.INVALID_NODE_NAME") : undefined;
  const keyError = !isReferenceKey(data.key) ? t("studio.validation.issues.INVALID_NODE_KEY") : undefined;
  const [pendingPluginVersion, setPendingPluginVersion] = useState<NodeManifest>();
  const removedInputs = pendingPluginVersion
    ? (pluginVersionContext?.connectedInputs ?? []).filter((name) => !pendingPluginVersion.inputPorts.some((port) => port.name === name))
    : [];
  const removedOutputs = pendingPluginVersion
    ? (pluginVersionContext?.connectedOutputs ?? []).filter((name) => !pendingPluginVersion.outputPorts.some((port) => port.name === name))
    : [];
  const unsupportedParameters = pendingPluginVersion?.parameterSchema.additionalProperties === false
    ? Object.keys(data.parameters).filter((name) => !(name in (pendingPluginVersion.parameterSchema.properties ?? {})))
    : [];
  const panelContext = manifest
    ? {
        data,
        manifest,
        localized,
        fieldErrors,
        providerOptions,
        referenceCatalog,
        parameterCatalogs,
        currentNodeCatalog,
        resources,
        workflowId,
  draftId,
        sourceNodeId,
        readOnly,
        onChange,
        onResourceAuthorize,
        onResourceRequest,
      }
    : undefined;
  const Panel =
    data.editorKind === "action" && panelContext
      ? resolveNodePanel(panelContext.manifest, data.nodeType)
      : undefined;
  return (
    <div className="space-y-5">
      <details className="rounded-md border border-border/60" data-testid="node-basics">
        <summary className="flex cursor-pointer items-center justify-between gap-2 px-2.5 py-2 text-[11px] text-muted-foreground">
          <span>{t("studio.inspector.basics")}</span>
          <span className="min-w-0 truncate font-medium text-foreground/70">{title}</span>
        </summary>
        <div className="space-y-3 border-t border-border/60 p-2.5">
          <PanelField error={fieldErrors.name ?? nameError} fieldPath="name" label={t("studio.inspector.name")} required>
            <Input
              disabled={readOnly}
              onChange={(event) => onChange({ label: event.target.value })}
              value={data.label.trim() ? title : ""}
            />
          </PanelField>
          <PanelField error={fieldErrors.key ?? keyError} fieldPath="key" label={t("studio.inspector.key")} required>
            <Input
              className="font-mono"
              disabled={readOnly}
              onChange={(event) =>
                onChange({
                  key: event.target.value.toLowerCase().replace(/[^a-z0-9_]/g, "_"),
                })
              }
              value={data.key}
            />
          </PanelField>
        </div>
      </details>
      {manifest?.plugin && pluginVersions.length > 1 && <PanelField fieldPath="typeVersion" label={t("studio.inspector.pluginVersion")}>
        <Select disabled={readOnly} onValueChange={(value) => {
          const selected = pluginVersions.find((candidate) => String(candidate.version) === value)
          if (selected) setPendingPluginVersion(selected)
        }} options={pluginVersions.map((candidate) => ({ value: String(candidate.version), label: `${candidate.plugin?.packageVersion ?? candidate.version} · ${candidate.plugin?.bundleDigest.slice(0, 19)}` }))} value={String(data.typeVersion)} />
      </PanelField>}
      <Dialog onOpenChange={(open) => { if (!open) setPendingPluginVersion(undefined) }} open={Boolean(pendingPluginVersion)}>
        <DialogContent description={t("studio.inspector.pluginVersionWarning")} title={t("studio.inspector.pluginVersionPreview")}>
          <div className="space-y-4 p-5 text-xs">
            <dl className="grid grid-cols-2 gap-3 rounded-md border border-border p-3">
              <Value label={t("studio.inspector.affectedPackageNodes")} value={pluginVersionContext?.affectedNodes ?? 1} />
              <Value label={t("studio.inspector.dependentReferences")} value={pluginVersionContext?.dependentReferences ?? 0} />
              <Value label={t("studio.inspector.removedInputPorts")} value={removedInputs.join(', ') || '—'} />
              <Value label={t("studio.inspector.removedOutputPorts")} value={removedOutputs.join(', ') || '—'} />
              <Value label={t("studio.inspector.unsupportedParameters")} value={unsupportedParameters.join(', ') || '—'} />
            </dl>
            <div className="flex justify-end gap-2"><Button onClick={() => setPendingPluginVersion(undefined)} variant="ghost">{t("common.cancel")}</Button><Button onClick={() => { if (pendingPluginVersion) onChange({ nodeType: pendingPluginVersion.nodeType, typeVersion: pendingPluginVersion.version }); setPendingPluginVersion(undefined) }}>{t("studio.inspector.confirmPluginVersion")}</Button></div>
          </div>
        </DialogContent>
      </Dialog>
      {Panel && panelContext ? (
        <Panel {...panelContext} />
      ) : (
        <div className="rounded-md border border-danger/30 bg-danger/5 p-3 text-xs text-danger" role="alert">
          {t("studio.inspector.unsupportedNodePanel").replace("{{nodeType}}", data.nodeType)}
        </div>
      )}
    </div>
  );
}

function Value({ label, value }: { label: string; value: ReactNode }) {
  return <div><dt className="text-muted-foreground">{label}</dt><dd className="mt-1 break-all font-medium">{value}</dd></div>;
}

function JsonValue({ value }: { value: unknown }) {
  return (
    <pre className="whitespace-pre-wrap break-all font-mono text-[11px] leading-5 text-muted-foreground">
      {JSON.stringify(value ?? {}, null, 2)}
    </pre>
  );
}

function addCurrentNodeReferences(
  catalog: ReferenceCatalog,
  data: ActionNodeData,
  manifest: NodeManifest,
  currentRawOutputLabel: string,
  nodeId?: string,
): ReferenceCatalog {
  const nativeSchema = asReferenceSchema(manifest.outputSchema);
  const itemPath = "item.json";
  const itemSelector = valueSelector("item");
  const itemFields = referenceFields(nativeSchema, itemPath, itemSelector);
  const nodePath = `outputs.${data.key}`;
  const ports = manifest.outputPorts.map((port) => {
    const portNativeSchema = asReferenceSchema(
      manifest.outputPortSchemas?.[port.name] ?? manifest.outputSchema,
    );
    const schema = portNativeSchema;
    const jsonPath = `${nodePath}.${port.name}.current.json`;
    const currentSelector = nodeId
      ? valueSelector("outputs", nodeId, port.name)
      : undefined;
    const fields = referenceFields(schema, jsonPath, currentSelector);
    const current: ReferenceEntry = {
      id: jsonPath,
      label: "current",
      path: jsonPath,
      selector: fields.length ? undefined : currentSelector,
      type: "object",
      schema,
      nullable: true,
      children: fields,
    };
    return {
      id: `${nodePath}.${port.name}`,
      label: port.name,
      path: `${nodePath}.${port.name}`,
      children: [current],
    } satisfies ReferenceEntry;
  });
  return {
    ...catalog,
    outputs: [
      ...catalog.outputs,
      { id: nodePath, label: data.key, path: nodePath, sourceNodeType: data.nodeType, children: ports },
    ],
    item: [
      {
        id: itemPath,
        label: currentRawOutputLabel,
        path: itemPath,
        selector: itemFields.length ? undefined : itemSelector,
        type: "object",
        schema: nativeSchema,
        nullable: true,
        children: itemFields,
      },
    ],
  };
}

function addCurrentItemReference(
  catalog: ReferenceCatalog,
  label: string,
): ReferenceCatalog {
  return {
    ...catalog,
    item: [{
      id: "item.json",
      label,
      path: "item.json",
      selector: valueSelector("item"),
      type: "object",
      children: [],
    }],
  };
}

type ReferenceSchema = JsonSchemaProperty;

function asReferenceSchema(value: unknown): ReferenceSchema {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as ReferenceSchema)
    : {};
}

function referenceFields(
  schema: ReferenceSchema,
  parent: string,
  parentSelector?: ValueSelector,
): ReferenceEntry[] {
  return Object.entries(schema.properties ?? {}).map(([name, child]) => {
    const path = `${parent}.${name}`;
    const selector = parentSelector
      ? { ...parentSelector, path: [...(parentSelector.path ?? []), name] }
      : undefined;
    const children = referenceFields(child, path, selector);
    return {
      id: path,
      label: name,
      path,
      selector: children.length ? undefined : selector,
      type: Array.isArray(child.type) ? child.type.join(" | ") : child.type,
      schema: child,
      nullable: !(schema.required ?? []).includes(name),
      children,
    };
  });
}

function valueSelector(
  namespace: ValueSelector["namespace"],
  sourceNodeId?: string,
  port?: string,
): ValueSelector {
  return {
    namespace,
    sourceNodeId,
    port,
    run: { kind: "current" },
    item: { kind: "current" },
    path: [],
  };
}

function parameterEntries(manifest?: NodeManifest) {
  return Object.entries(manifest?.parameterSchema.properties ?? {});
}
function uiFields(manifest?: NodeManifest): Record<string, UiField> {
  return manifest?.uiSchema.fields ?? {};
}
