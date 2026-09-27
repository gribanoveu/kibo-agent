import { useEffect, useRef, useState } from "react";
import type { ChatUsage, ContextUsage } from "../lib/chat";
import "./ContextMeter.css";

// How full the model's window is, as a ring beside the send button; a click opens
// what the tokens are spent on and the one thing to do about it — fold the
// older part of the conversation into a summary.
//
// The parts are shown apart because they behave differently: folding moves
// the conversation and leaves the rest exactly where it was, and skills and
// MCP servers are the user's to switch off — each row is something to do.
// This is the backend's estimate; the provider's own count for the last
// request goes next to it rather than implying the two agree.

const compact = (n: number) => (n >= 1000 ? `${Math.round(n / 1000)}k` : `${n}`);

// Zero is also what a provider that reports nothing sends, so it is not
// called a miss.
const cacheNote = ({ promptTokens, cachedTokens = 0 }: ChatUsage) =>
  cachedTokens && promptTokens
    ? `, ${compact(cachedTokens)} of it (${Math.round((cachedTokens / promptTokens) * 100)}%) from the cache`
    : ", none of it reported as cached";

// In the order of the bar: the fixed parts first, the conversation last.
const PARTS = [
  { key: "instructions", label: "Instructions and rules" },
  { key: "tools", label: "Built-in tools" },
  { key: "skills", label: "Skills" },
  { key: "mcp", label: "MCP servers" },
  { key: "conversation", label: "Conversation" },
] as const;

const R = 7;
const CIRCUMFERENCE = 2 * Math.PI * R;

type Props = {
  context: ContextUsage;
  usage: ChatUsage | null;
  /** Mid-turn the history is the turn's: folding it from under a running request is not offered. */
  running: boolean;
  onCompact: () => void;
  /** Opens the panel upwards — for a meter at the bottom of the window. */
  up?: boolean;
};

export function ContextMeter({ context, usage, running, onCompact, up = false }: Props) {
  const [open, setOpen] = useState(false);
  const wrap = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: PointerEvent) => {
      if (!wrap.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    document.addEventListener("pointerdown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("pointerdown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  // No window is a number with no scale — the ring stays empty rather than
  // filling against a guess.
  const share = context.limit ? Math.min(1, context.total / context.limit) : null;
  const percent = share === null ? null : Math.round(share * 100);
  const tone = share === null || share < 0.7 ? "ok" : share < 0.9 ? "warn" : "full";

  return (
    <div className="ctx" ref={wrap}>
      <button
        type="button"
        className={`ctx-ring ctx-ring--${tone}${open ? " open" : ""}`}
        aria-label={percent === null ? "Context usage" : `Context usage: ${percent}%`}
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
      >
        <svg width="18" height="18" viewBox="0 0 18 18" aria-hidden="true">
          <circle className="ctx-track" cx="9" cy="9" r={R} />
          {share !== null && (
            <circle
              className="ctx-arc"
              cx="9"
              cy="9"
              r={R}
              strokeDasharray={CIRCUMFERENCE}
              strokeDashoffset={CIRCUMFERENCE * (1 - share)}
            />
          )}
        </svg>
      </button>

      {open && (
        <div className={`ctx-pop${up ? " up" : ""}`} role="dialog" aria-label="Context">
          <div className="ctx-head">
            <span>Context window</span>
            {percent !== null && <span className="ctx-percent">{percent}%</span>}
          </div>

          {context.limit ? (
            <>
              <div className="ctx-bar" aria-hidden="true">
                {PARTS.map(({ key }) => (
                  <span
                    key={key}
                    className={`ctx-part--${key}`}
                    style={{ width: `${(context[key] / (context.limit ?? 1)) * 100}%` }}
                  />
                ))}
              </div>
              <p className="ctx-total">
                {compact(context.total)} of {compact(context.limit)} tokens
              </p>
            </>
          ) : (
            <p className="ctx-total">
              About {compact(context.total)} tokens in the next request. The model's window is not known —
              set it in Settings → Models.
            </p>
          )}

          <dl className="ctx-rows">
            {PARTS.map(({ key, label }) => (
              <div key={key}>
                <dt>
                  <i className={`ctx-dot ctx-part--${key}`} />
                  {label}
                </dt>
                <dd>{compact(context[key])}</dd>
              </div>
            ))}
          </dl>

          {context.compactsAt && (
            <p className="ctx-note">Folds the older part on its own at {compact(context.compactsAt)}.</p>
          )}
          {usage && (
            <p className="ctx-note">
              The last request actually cost {compact(usage.promptTokens)}
              {cacheNote(usage)}.
            </p>
          )}

          <button
            type="button"
            className="btn btn-ghost ctx-compact"
            disabled={running}
            title={running ? "Not while a turn is running" : undefined}
            onClick={() => {
              setOpen(false);
              onCompact();
            }}
          >
            Compact now
          </button>
        </div>
      )}
    </div>
  );
}
