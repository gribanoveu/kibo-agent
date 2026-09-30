import { useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { MessageCircleQuestion } from "lucide-react";
import { Dropdown } from "./Dropdown";
import type { McpAnswer, McpField } from "../lib/chat";
import type { Block } from "../lib/chatTurnReducer";
import "./McpQuestionCard.css";

type Question = Extract<Block, { kind: "question" }>;

/** What a field holds while the form is filled in: what was typed, not yet the value sent. */
type Draft = Record<string, string>;

const drafted = (fields: McpField[]): Draft =>
  Object.fromEntries(fields.map((field) => [field.name, field.default == null ? "" : String(field.default)]));

/** The draft as the server's values; `null` while a required field is empty or a number is not one. */
export function formValues(fields: McpField[], draft: Draft): Record<string, unknown> | null {
  const values: Record<string, unknown> = {};
  for (const field of fields) {
    const typed = (draft[field.name] ?? "").trim();
    if (!typed) {
      if (field.required) return null;
      continue;
    }
    switch (field.kind.type) {
      case "number": {
        const n = Number(typed);
        if (!Number.isFinite(n) || (field.kind.integer && !Number.isInteger(n))) return null;
        values[field.name] = n;
        break;
      }
      case "boolean":
        values[field.name] = typed === "true";
        break;
      default:
        values[field.name] = typed;
    }
  }
  return values;
}

/**
 * An MCP server asking the user something in the middle of a call. The call
 * waits for the answer; `live` is false once the turn is over, and a card
 * left open then can no longer be answered.
 */
export function McpQuestionCard({
  block,
  live,
  onAnswer,
}: {
  block: Question;
  live: boolean;
  onAnswer?: (id: string, answer: McpAnswer) => void;
}) {
  const { question, server } = block;
  const [draft, setDraft] = useState<Draft>(() => (question.mode === "form" ? drafted(question.fields) : {}));
  const open = block.status === "open" && live && Boolean(onAnswer);
  const answer = (given: McpAnswer) => onAnswer?.(block.id, given);

  if (block.status !== "open" || !live) {
    const said = { accept: "answered", decline: "declined", cancel: "left unanswered", open: "left unanswered" }[block.status];
    return (
      <div className="mcp-question closed">
        <div className="mcp-question-label">
          <MessageCircleQuestion size={13} aria-hidden /> {server} asked · {said}
        </div>
        <div className="mcp-question-message">{question.message}</div>
      </div>
    );
  }

  const values = question.mode === "form" ? formValues(question.fields, draft) : null;
  return (
    <div className="mcp-question">
      <div className="mcp-question-label">
        <MessageCircleQuestion size={13} aria-hidden /> {server} asks — the call waits for your answer
      </div>
      <div className="mcp-question-message">{question.message}</div>
      {question.mode === "url" ? (
        <>
          <div className="mcp-question-url">{question.url}</div>
          <div className="mcp-question-note">Opens in your browser. The server is told only that you agreed.</div>
        </>
      ) : (
        question.fields.map((field) => (
          // A <div>, not a <label>: a label names every control inside it, and
          // the two answers of a yes-or-no field would both be called by it.
          <div className="mcp-question-field" key={field.name}>
            <span className="mcp-question-name">
              {field.label}
              {field.required && <span className="mcp-question-required"> *</span>}
            </span>
            <FieldInput field={field} value={draft[field.name] ?? ""} onChange={(v) => setDraft((d) => ({ ...d, [field.name]: v }))} />
            {field.description && <span className="mcp-question-note">{field.description}</span>}
          </div>
        ))
      )}
      <div className="mcp-question-actions">
        {question.mode === "url" ? (
          <button
            type="button"
            className="btn primary"
            disabled={!open}
            onClick={() => {
              void openUrl(question.url).catch(() => {});
              answer({ action: "accept" });
            }}
          >
            Open in browser
          </button>
        ) : (
          <button type="button" className="btn primary" disabled={!open || values === null} onClick={() => values && answer({ action: "accept", content: values })}>
            Send
          </button>
        )}
        <button type="button" className="btn" disabled={!open} onClick={() => answer({ action: "decline" })}>
          Decline
        </button>
      </div>
    </div>
  );
}

function FieldInput({ field, value, onChange }: { field: McpField; value: string; onChange: (value: string) => void }) {
  switch (field.kind.type) {
    case "boolean":
      return (
        <div className="mcp-question-choice" role="radiogroup" aria-label={field.label}>
          {[
            ["Yes", "true"],
            ["No", "false"],
          ].map(([label, v]) => (
            <button key={v} type="button" role="radio" aria-checked={value === v} className={value === v ? "active" : ""} onClick={() => onChange(v)}>
              {label}
            </button>
          ))}
        </div>
      );
    case "choice": {
      const { values, labels } = field.kind;
      const options = values.map((v, i) => ({ value: v, label: labels[i] || v }));
      return (
        <Dropdown
          label={options.find((o) => o.value === value)?.label ?? "Choose…"}
          title={field.label}
          options={options}
          value={value}
          onPick={onChange}
        />
      );
    }
    case "number":
      return (
        <input
          className="mcp-question-input"
          inputMode={field.kind.integer ? "numeric" : "decimal"}
          value={value}
          aria-label={field.label}
          onChange={(e) => onChange(e.target.value)}
        />
      );
    default:
      return <input className="mcp-question-input" value={value} aria-label={field.label} onChange={(e) => onChange(e.target.value)} />;
  }
}
