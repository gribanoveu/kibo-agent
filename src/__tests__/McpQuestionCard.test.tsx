import { beforeEach, describe, expect, mock, test } from "bun:test";
import { fireEvent, render, screen } from "@testing-library/react";
import type { McpAnswer, McpField, McpQuestion } from "../lib/chat";
import type { Block } from "../lib/chatTurnReducer";

const opened: string[] = [];
mock.module("@tauri-apps/plugin-opener", () => ({ openUrl: async (url: string) => void opened.push(url) }));

const { McpQuestionCard, formValues } = await import("../components/McpQuestionCard");

const field = (name: string, kind: McpField["kind"], extra: Partial<McpField> = {}): McpField => ({
  name,
  label: name,
  description: null,
  required: false,
  kind,
  default: null,
  ...extra,
});

const card = (question: McpQuestion, status: Extract<Block, { kind: "question" }>["status"] = "open", live = true) => {
  const answers: [string, McpAnswer][] = [];
  render(
    <McpQuestionCard
      block={{ kind: "question", id: "q1", round: 1, call: "c1", server: "tracker", question, status }}
      live={live}
      onAnswer={(id, answer) => answers.push([id, answer])}
    />,
  );
  return answers;
};

beforeEach(() => {
  opened.length = 0;
});

describe("the values a form sends", () => {
  const fields = [
    field("repo", { type: "text" }, { required: true }),
    field("count", { type: "number", integer: true }),
    field("ratio", { type: "number", integer: false }),
    field("force", { type: "boolean" }),
  ];

  test("are what was filled in, typed as the server asked, and nothing for what was left empty", () => {
    expect(formValues(fields, { repo: " a/b ", count: "3", ratio: "0.5", force: "false" })).toEqual({
      repo: "a/b",
      count: 3,
      ratio: 0.5,
      force: false,
    });
    expect(formValues(fields, { repo: "a/b", count: "", ratio: "", force: "" })).toEqual({ repo: "a/b" });
  });

  test("are not ready while a required field is empty or a number is not one", () => {
    expect(formValues(fields, { repo: "" })).toBeNull();
    expect(formValues(fields, { repo: "a/b", count: "2.5" })).toBeNull();
    expect(formValues(fields, { repo: "a/b", ratio: "many" })).toBeNull();
  });
});

describe("a server's question", () => {
  test("is a form that sends what was filled in, and only once it can", () => {
    const answers = card({
      mode: "form",
      message: "Which repository?",
      fields: [field("repo", { type: "text" }, { required: true, label: "Repository" }), field("count", { type: "number", integer: true }, { default: 3 })],
    });
    expect(screen.getByText("Which repository?")).toBeTruthy();
    expect(screen.getByText(/tracker asks/)).toBeTruthy();
    const send = screen.getByText("Send") as HTMLButtonElement;
    expect(send.disabled).toBe(true, "a required field is empty");

    fireEvent.change(screen.getByLabelText("Repository"), { target: { value: "a/b" } });
    fireEvent.click(screen.getByText("Send"));
    expect(answers).toEqual([["q1", { action: "accept", content: { repo: "a/b", count: 3 } }]]);
  });

  test("a yes-or-no field and a choice are the app's own controls", () => {
    const answers = card({
      mode: "form",
      message: "?",
      fields: [field("force", { type: "boolean" }), field("branch", { type: "choice", values: ["main", "dev"], labels: ["Main", ""] })],
    });
    fireEvent.click(screen.getByRole("radio", { name: "Yes" }));
    fireEvent.click(screen.getByText("Choose…"));
    fireEvent.click(screen.getByRole("option", { name: /dev/ }));
    fireEvent.click(screen.getByText("Send"));
    expect(answers[0][1]).toEqual({ action: "accept", content: { force: true, branch: "dev" } });
  });

  test("can be declined", () => {
    const answers = card({ mode: "form", message: "?", fields: [] });
    fireEvent.click(screen.getByText("Decline"));
    expect(answers).toEqual([["q1", { action: "decline" }]]);
  });

  test("an address to open is shown whole, and opened only by the click that agrees", () => {
    const answers = card({ mode: "url", message: "Sign in to the tracker", url: "https://tracker.example/login?x=1" });
    expect(screen.getByText("https://tracker.example/login?x=1")).toBeTruthy();
    expect(opened).toEqual([]);
    fireEvent.click(screen.getByText("Open in browser"));
    expect(opened).toEqual(["https://tracker.example/login?x=1"]);
    expect(answers).toEqual([["q1", { action: "accept" }]]);
  });

  test("once answered, or once its turn is over, says so and offers nothing", () => {
    card({ mode: "form", message: "Which?", fields: [] }, "decline");
    expect(screen.getByText(/tracker asked · declined/)).toBeTruthy();
    expect(screen.queryByText("Send")).toBeNull();
  });

  test("left open when the turn ended cannot be answered", () => {
    card({ mode: "url", message: "Sign in", url: "https://a.example" }, "open", false);
    expect(screen.getByText(/left unanswered/)).toBeTruthy();
    expect(screen.queryByText("Open in browser")).toBeNull();
  });
});
