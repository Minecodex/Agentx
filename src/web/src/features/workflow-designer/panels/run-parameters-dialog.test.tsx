import * as TooltipPrimitive from "@radix-ui/react-tooltip";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { WorkflowStart } from "../model/types";
import {
  RunParametersDialog,
  hasRunParameters,
} from "./run-parameters-dialog";

const start: WorkflowStart = {
  inputs: {
    type: "object",
    required: ["question"],
    properties: {
      question: {
        type: "string",
        title: "Question",
        description: "Question sent to the model",
        minLength: 3,
      },
    },
  },
  contexts: {
    apiKey: {
      title: "API key",
      description: "Temporary credential",
      schema: { type: "string" },
      default: null,
      mutable: false,
      sensitive: true,
      clientWritable: true,
      scope: "execution_tree",
      mergePolicy: "replace",
    },
    internal: {
      title: "Internal",
      schema: { type: "string" },
      default: "system",
      mutable: false,
      sensitive: false,
      clientWritable: false,
      scope: "execution_tree",
      mergePolicy: "replace",
    },
  },
};

describe("RunParametersDialog", () => {
  it("renders schema fields, tooltips and required validation", async () => {
    const onRun = vi.fn();
    render(
      <TooltipPrimitive.Provider delayDuration={0}>
        <RunParametersDialog
          onClose={vi.fn()}
          onRun={onRun}
          open
          running={false}
          start={start}
        />
      </TooltipPrimitive.Provider>,
    );

    expect(screen.queryByLabelText("Internal")).not.toBeInTheDocument();
    expect(screen.getByLabelText("API key")).toHaveAttribute("type", "password");
    fireEvent.focus(screen.getByText("Question"));
    expect(await screen.findByText("Question sent to the model")).toBeVisible();

    fireEvent.click(screen.getByRole("button", { name: "Run workflow" }));
    expect(await screen.findByText("This field is required")).toBeVisible();
    expect(onRun).not.toHaveBeenCalled();

    fireEvent.change(screen.getByLabelText("Question"), {
      target: { value: "What changed?" },
    });
    fireEvent.change(screen.getByLabelText("API key"), {
      target: { value: "secret" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Run workflow" }));

    await waitFor(() =>
      expect(onRun).toHaveBeenCalledWith(
        { question: "What changed?" },
        { apiKey: "secret" },
        {},
      ),
    );
  });

  it("skips the dialog only when no caller values are declared", () => {
    expect(hasRunParameters(start)).toBe(true);
    expect(
      hasRunParameters({
        inputs: { type: "object", properties: {} },
        contexts: {},
      }),
    ).toBe(false);
  });
});
