import { describe, expect, it } from "vitest";

import type { StudioDocument } from "../model/types";
import { definitionIssues, isInputBindingEmpty, isReferenceKey } from "./definition-validation";

const document = (): StudioDocument => ({
  start: { inputs: { type: "object" }, contexts: {} },
  nodes: [],
  edges: [],
  end: { completion: "first_return", outputs: {}, error: { outputs: { } } },
  boundaryLayouts: [],
  viewport: { x: 0, y: 0, zoom: 1 },
  annotations: [],
  groups: [],
  settings: { executionOrder: "deterministic", activationBudget: 10_000 },
});

describe("workflow definition validation", () => {
  it("matches the backend reference-key contract", () => {
    expect(isReferenceKey("answer_2")).toBe(true);
    expect(isReferenceKey("_answer")).toBe(true);
    expect(isReferenceKey("2answer")).toBe(false);
    expect(isReferenceKey("Answer")).toBe(false);
    expect(isReferenceKey("中文")).toBe(false);
  });

  it("distinguishes missing End output content from valid falsey values and references", () => {
    expect(isInputBindingEmpty({ kind: "literal", value: "  " })).toBe(true);
    expect(isInputBindingEmpty({ kind: "literal", value: 0 })).toBe(false);
    expect(isInputBindingEmpty({ kind: "literal", value: false })).toBe(false);
    expect(isInputBindingEmpty({ kind: "reference", selector: { namespace: "inputs", run: { kind: "current" }, item: { kind: "current" }, path: ["question"] }, missingPolicy: { kind: "error" } })).toBe(false);
  });

  it("finds user-editable values rejected by backend definition validation", () => {
    const value = document();
    value.settings.activationBudget = 0;
    value.start.contexts["Bad-name"] = { schema: { type: "string" }, default: "", mutable: true, sensitive: false, clientWritable: false, scope: "execution_tree", maxSize: 0, mergePolicy: "replace" };
    value.end.outputs["中文"] = { schema: { type: "string" }, required: false, sensitive: false };
    value.end.outputs["answer"] = { schema: { type: "string" }, required: false, sensitive: false };
    value.nodes = [{ id: "exit", type: "exit", position: { x: 0, y: 0 }, data: { editorKind: "exit", key: "exit", label: "End", protected: false, parameters: { outputs: { answer: { kind: "literal", value: "  " } }, errorOutputs: {} } } }];

    expect(definitionIssues(value).map((issue) => issue.code)).toEqual(expect.arrayContaining([
      "INVALID_ACTIVATION_BUDGET",
      "INVALID_CONTEXT_KEY",
      "INVALID_CONTEXT_MAX_SIZE",
      "INVALID_END_OUTPUT_KEY",
      "END_OUTPUT_BINDING_REQUIRED",
    ]));
  });

  it("keeps end node mappings aligned with the shared contract", () => {
    const value = document();
    value.end.outputs["answer"] = { schema: { type: "string" }, required: true, sensitive: false };
    value.nodes = [{ id: "exit", type: "exit", position: { x: 0, y: 0 }, data: { editorKind: "exit", key: "exit", label: "End", protected: false, parameters: { outputs: { ghost: { kind: "literal", value: "x" } }, errorOutputs: {} } } }];

    value.edges = [{ id: "start-exit", source: "__start__", sourceHandle: "main", target: "exit", targetHandle: "main", data: { edgeKind: "execution", order: 0 } }];
    expect(definitionIssues(value).map((issue) => issue.code)).toEqual(expect.arrayContaining([
      "EXIT_MAPPING_KEY_UNKNOWN",
      "EXIT_REQUIRED_MAPPING_MISSING",
    ]));
  });

  it("rejects a document without any end node", () => {
    expect(definitionIssues(document()).map((issue) => issue.code)).toContain("EXIT_REQUIRED");
  });

  it("allows an incomplete Agent draft while rejecting duplicate model bindings", () => {
    const value = document();
    value.nodes = [{ id: "exit", type: "exit", position: { x: 0, y: 0 }, data: { editorKind: "exit", key: "exit", label: "End", protected: false, parameters: { outputs: {}, errorOutputs: {} } } }, {
      id: "agent", type: "manifest", position: { x: 100, y: 0 },
      data: { editorKind: "action", nodeType: "agent", typeVersion: 2, key: "agent", label: "Agent", parameters: { sessionPolicy: { mode: "invocation" } }, contextWrites: [], settings: {}, disabled: false, resourceReferences: [] },
    }];
    expect(definitionIssues(value)).toEqual([]);
    const data = value.nodes[1].data;
    if (data.editorKind !== "action") throw new Error("Expected Agent action");
    const references = data.resourceReferences;
    references.push(...[1, 2].map(id => ({ resourceType: "model" as const, bindingRole: "model", operation: "use" as const, resourceId: `model-${id}`, resourceVersionId: `version-${id}` })));
    expect(definitionIssues(value).map(issue => issue.code)).toContain("AGENT_RESOURCE_SLOT_INVALID");
  });

  it("checks generated node and connection invariants before sending a draft", () => {
    const value = document();
    value.start.inputs = [] as never;
    value.nodes = [{
      id: "__end__",
      type: "manifest",
      position: { x: 0, y: 0 },
      data: {
        editorKind: "action",
        nodeType: "Bad Type",
        typeVersion: 0,
        label: " ",
        key: "Bad-key",
        parameters: {},
        contextWrites: [],
        resourceReferences: [],
        settings: { maxTries: 0 },
        disabled: false,
      },
    }];
    value.edges = [{ id: "", source: "__end__", sourceHandle: "main", target: "missing", targetHandle: "main", data: { edgeKind: "execution", order: 0 } }];

    expect(definitionIssues(value).map((issue) => issue.code)).toEqual(expect.arrayContaining([
      "INVALID_INPUT_SCHEMA",
      "RESERVED_NODE_ID",
      "INVALID_NODE_KEY",
      "INVALID_NODE_TYPE",
      "INVALID_NODE_NAME",
      "UNSUPPORTED_NODE_VERSION",
      "INVALID_MAX_TRIES",
      "INVALID_CONNECTION_ID",
      "END_BOUNDARY_REMOVED",
    ]));
  });
});

 it("allows an error-only Exit without success mappings and enforces its error contract", () => {
    const value = document();
    value.end.outputs.answer = { schema: { type: "string" }, required: true, sensitive: false };
    value.end.error.outputs.message = { schema: { type: "string" }, required: true, sensitive: false };
    value.nodes = [{ id: "exit", type: "exit", position: { x: 0, y: 0 }, data: { editorKind: "exit", key: "exit", label: "Error", protected: false, parameters: { outputs: {}, errorOutputs: { message: { kind: "reference", selector: { namespace: "item", path: ["message"] } } } } } }];
    value.edges = [{ id: "error-exit", source: "source", sourceHandle: "error", target: "exit", targetHandle: "error", data: { edgeKind: "execution", order: 0 } }];
    expect(definitionIssues(value).some(issue => issue.code === "EXIT_REQUIRED_MAPPING_MISSING")).toBe(false);
    if (value.nodes[0]!.data.editorKind === 'exit') value.nodes[0]!.data.parameters.errorOutputs = {};
    expect(definitionIssues(value).filter(issue => issue.code === "EXIT_REQUIRED_MAPPING_MISSING").map(issue => issue.fieldPath)).toEqual(["parameters.errorOutputs.message"]);
  });
