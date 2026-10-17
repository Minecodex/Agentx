import { LexicalComposer } from "@lexical/react/LexicalComposer";
import { ContentEditable } from "@lexical/react/LexicalContentEditable";
import { LexicalErrorBoundary } from "@lexical/react/LexicalErrorBoundary";
import { HistoryPlugin } from "@lexical/react/LexicalHistoryPlugin";
import { OnChangePlugin } from "@lexical/react/LexicalOnChangePlugin";
import { PlainTextPlugin } from "@lexical/react/LexicalPlainTextPlugin";
import { useLexicalComposerContext } from "@lexical/react/LexicalComposerContext";
import {
  $createParagraphNode,
  $createTextNode,
  $getRoot,
  $getSelection,
  $insertNodes,
  $isRangeSelection,
  $isTextNode,
  DecoratorNode,
  type EditorConfig,
  type ElementNode,
  type LexicalEditor,
  type LexicalNode,
  type NodeKey,
  type SerializedLexicalNode,
  type Spread,
} from "lexical";
import { useEffect, useRef } from "react";

import type {
  MissingValuePolicy,
  ReferenceCatalog,
  ReferenceEntry,
  TemplateBinding,
  InputTemplateSegment,
  ValueSelector,
} from "../model/types";
import { selectorsEqual } from "../model/selector";
import { DEFAULT_REFERENCE_COLOR, sourceNodeColor } from "./reference-picker/reference-color";
import { useReadOnlyForm } from "./read-only-form";

const AGENTX_TEMPLATE_MIME = "application/x-agentx-text-template+json";

type SerializedVariableNode = Spread<
  { selector: ValueSelector; missingPolicy: MissingValuePolicy; label?: string; color?: string },
  SerializedLexicalNode
>;

class VariableNode extends DecoratorNode<React.ReactNode> {
  __selector: ValueSelector;
  __missingPolicy: MissingValuePolicy;
  __label?: string;
  __color?: string;

  static getType() {
    return "agentx-variable";
  }

  static clone(node: VariableNode) {
    return new VariableNode(
      structuredClone(node.__selector),
      structuredClone(node.__missingPolicy),
      node.__label,
      node.__color,
      node.__key,
    );
  }

  static importJSON(value: SerializedVariableNode) {
    return new VariableNode(value.selector, value.missingPolicy, value.label, value.color);
  }

  constructor(
    selector: ValueSelector,
    missingPolicy: MissingValuePolicy,
    label?: string,
    color?: string,
    key?: NodeKey,
  ) {
    super(key);
    this.__selector = selector;
    this.__missingPolicy = missingPolicy;
    this.__label = label;
    this.__color = color;
  }

  createDOM(_config: EditorConfig) {
    const element = document.createElement("span");
    element.className = "inline-flex align-middle";
    return element;
  }

  updateDOM() {
    return false;
  }

  isInline() {
    return true;
  }

  isKeyboardSelectable() {
    return false;
  }

  exportJSON(): SerializedVariableNode {
    return {
      ...super.exportJSON(),
      selector: structuredClone(this.__selector),
      missingPolicy: structuredClone(this.__missingPolicy),
      label: this.__label,
      color: this.__color,
      type: "agentx-variable",
      version: 1,
    };
  }

  decorate() {
    const color = this.__color ?? DEFAULT_REFERENCE_COLOR;
    return (
      <span
        className="mx-0.5 inline-flex h-6 max-w-56 select-none items-center gap-1.5 rounded-md border border-primary/25 bg-primary/10 px-1.5 align-middle text-xs font-medium text-primary"
        data-agentx-variable
        data-agentx-variable-color={color}
        title={this.__label ?? fallbackSelectorLabel(this.__selector)}
      >
        <span className="size-1.5 shrink-0 rounded-full" style={{ backgroundColor: color }} />
        <span className="truncate">{this.__label ?? fallbackSelectorLabel(this.__selector)}</span>
      </span>
    );
  }
}

const $createVariableNode = (
  selector: ValueSelector,
  label?: string,
  color?: string,
  missingPolicy: MissingValuePolicy = { kind: "error" },
) => new VariableNode(structuredClone(selector), structuredClone(missingPolicy), label, color);
const $isVariableNode = (node: LexicalNode | null | undefined): node is VariableNode =>
  node instanceof VariableNode;

export function VariableTokenEditor({
  value,
  onChange,
  multiline = false,
  catalog,
  onEditorReady,
}: {
  value: TemplateBinding;
  onChange: (value: TemplateBinding) => void;
  multiline?: boolean;
  catalog?: ReferenceCatalog;
  onEditorReady?: (editor: LexicalEditor) => void;
}) {
  const readOnly = useReadOnlyForm();
  const current = useRef(value);
  current.current = value;
  return (
    <LexicalComposer
      initialConfig={{
        namespace: "agentx-variable-token-editor",
        editable: !readOnly,
        nodes: [VariableNode],
        onError: (error) => {
          throw error;
        },
        editorState: () => replaceEditorValue(value, catalog),
        theme: { paragraph: "m-0" },
      }}
    >
      <div
        className={`relative rounded-md border border-border bg-surface text-xs focus-within:border-primary focus-within:ring-2 focus-within:ring-primary/15 ${multiline ? "min-h-24" : "min-h-9"}`}
        data-testid="variable-token-editor"
      >
        <PlainTextPlugin
          contentEditable={
            <ContentEditable
              aria-label="Value"
              className={`outline-none ${multiline ? "min-h-24 px-2 py-2" : "min-h-9 px-2 py-1.5 leading-6"}`}
            />
          }
          ErrorBoundary={LexicalErrorBoundary}
          placeholder={
            <span className="pointer-events-none absolute left-2 top-2 text-muted-foreground">
              选择变量或输入固定值
            </span>
          }
        />
        <HistoryPlugin />
        <OnChangePlugin
          ignoreSelectionChange
          onChange={(state) => {
            if (readOnly) return;
            state.read(() => {
              const next = editorValue();
              if (JSON.stringify(next) !== JSON.stringify(current.current)) onChange(next);
            });
          }}
        />
        <EditorBridge catalog={catalog} onReady={onEditorReady} value={value} />
      </div>
    </LexicalComposer>
  );
}

export function insertVariable(
  editor: LexicalEditor,
  selector: ValueSelector,
  label?: string,
  color?: string,
  trigger?: "{{" | "/",
) {
  if (!editor.isEditable()) return;
  editor.update(() => {
    const variable = $createVariableNode(selector, label, color);
    const selection = $getSelection();
    if ($isRangeSelection(selection)) {
      if (trigger && selection.isCollapsed()) {
        const node = selection.anchor.getNode();
        const offset = selection.anchor.offset;
        if ($isTextNode(node) && node.getTextContent().slice(0, offset).endsWith(trigger)) {
          node.spliceText(offset - trigger.length, trigger.length, "", true);
        }
      }
      $insertNodes([variable]);
      return;
    }
    let paragraph = $getRoot().getLastChild() as ElementNode | null;
    if (!paragraph) {
      paragraph = $createParagraphNode();
      $getRoot().append(paragraph);
    }
    paragraph.append(variable);
  });
  editor.focus();
}

function EditorBridge({
  value,
  catalog,
  onReady,
}: {
  value: TemplateBinding;
  catalog?: ReferenceCatalog;
  onReady?: (editor: LexicalEditor) => void;
}) {
  const [editor] = useLexicalComposerContext();
  const readOnly = useReadOnlyForm();
  useEffect(() => { editor.setEditable(!readOnly); }, [editor, readOnly]);
  const lastExternal = useRef(JSON.stringify(value));
  useEffect(() => onReady?.(editor), [editor, onReady]);
  useEffect(() => {
    const serialized = JSON.stringify(value);
    editor.getEditorState().read(() => {
      if (JSON.stringify(editorValue()) === serialized) {
        lastExternal.current = serialized;
        return;
      }
      if (lastExternal.current === serialized) return;
      editor.update(() => replaceEditorValue(value, catalog));
      lastExternal.current = serialized;
    });
  }, [catalog, editor, value]);
  useEffect(() => {
    const root = editor.getRootElement();
    if (!root) return;
    const copy = (event: ClipboardEvent) => {
      event.clipboardData?.setData(AGENTX_TEMPLATE_MIME, JSON.stringify(editorValueRead(editor)));
    };
    const paste = (event: ClipboardEvent) => {
      if (!editor.isEditable()) { event.preventDefault(); return; }
      const encoded = event.clipboardData?.getData(AGENTX_TEMPLATE_MIME);
      if (!encoded) return;
      try {
        const template = JSON.parse(encoded) as TemplateBinding;
        event.preventDefault();
        editor.update(() => {
          const selection = $getSelection();
          if ($isRangeSelection(selection)) selection.removeText();
          $insertNodes(nodesForValue(template));
        });
      } catch {
        // Ignore malformed external clipboard payloads and keep plain-text paste.
      }
    };
    root.addEventListener("copy", copy);
    root.addEventListener("cut", copy);
    root.addEventListener("paste", paste);
    return () => {
      root.removeEventListener("copy", copy);
      root.removeEventListener("cut", copy);
      root.removeEventListener("paste", paste);
    };
  }, [editor]);
  return null;
}

function editorValueRead(editor: LexicalEditor) {
  let value: TemplateBinding = { kind: "template", segments: [] };
  editor.getEditorState().read(() => {
    value = editorValue();
  });
  return value;
}

function editorValue(): TemplateBinding {
  const paragraph = $getRoot().getFirstChild();
  const segments: InputTemplateSegment[] = [];
  for (const node of (paragraph as ElementNode | null)?.getChildren() ?? []) {
    if ($isVariableNode(node)) {
      segments.push({
        kind: "reference",
        selector: structuredClone(node.__selector),
        missingPolicy: structuredClone(node.__missingPolicy),
      });
    } else if ($isTextNode(node) && node.getTextContent()) {
      const previous = segments.at(-1);
      if (previous?.kind === "text") previous.text += node.getTextContent();
      else segments.push({ kind: "text", text: node.getTextContent() });
    }
  }
  return { kind: "template", segments };
}

function replaceEditorValue(value: TemplateBinding, catalog?: ReferenceCatalog) {
  const root = $getRoot();
  root.clear();
  const paragraph = $createParagraphNode();
  paragraph.append(...nodesForValue(value, catalog));
  root.append(paragraph);
}

function nodesForValue(value: TemplateBinding, catalog?: ReferenceCatalog): LexicalNode[] {
  return value.segments.map((segment) =>
    segment.kind === "text"
      ? $createTextNode(segment.text)
      : $createVariableNode(segment.selector, selectorDisplayLabel(segment.selector, catalog), selectorDisplayColor(segment.selector, catalog), segment.missingPolicy),
  );
}

function fallbackSelectorLabel(selector: ValueSelector) {
  const source = selector.namespace === "outputs"
    ? selector.sourceNodeId?.slice(0, 8) ?? "output"
    : selector.namespace;
  return [source, selector.port, ...(selector.path ?? [])].filter(Boolean).join(" / ");
}

const STRUCTURAL_LABELS = ["current", "first", "last", "all()", "runs"];

export function selectorDisplayLabel(selector: ValueSelector, catalog?: ReferenceCatalog) {
  const chain = catalog ? findEntryChain(catalog, selector) : undefined;
  if (!chain) return fallbackSelectorLabel(selector);
  if (selector.namespace === "outputs") {
    const nodeName = chain[0]?.label;
    const field = [...chain]
      .slice(1)
      .reverse()
      .find((entry) => !STRUCTURAL_LABELS.includes(entry.label));
    return field ? `${nodeName} · ${field.label}` : nodeName ?? fallbackSelectorLabel(selector);
  }
  return chain
    .map((entry) => entry.label)
    .filter((label) => !STRUCTURAL_LABELS.includes(label))
    .join(" / ");
}

export function selectorDisplayColor(selector: ValueSelector, catalog?: ReferenceCatalog) {
  const chain = catalog ? findEntryChain(catalog, selector) : undefined;
  return sourceNodeColor(chain?.[0]?.sourceNodeType);
}

function findEntryChain(catalog: ReferenceCatalog, selector: ValueSelector): ReferenceEntry[] | undefined {
  const visit = (entry: ReferenceEntry, parents: ReferenceEntry[]): ReferenceEntry[] | undefined => {
    const chain = [...parents, entry];
    if (entry.selector && selectorsEqual(entry.selector, selector)) return chain;
    for (const child of entry.children) {
      const found = visit(child, chain);
      if (found) return found;
    }
    return undefined;
  };
  for (const entries of Object.values(catalog)) {
    for (const entry of entries ?? []) {
      const found = visit(entry, []);
      if (found) return found;
    }
  }
  return undefined;
}
