import { type CSSProperties, type ReactNode, useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import type { ChatUsage, ContextUsage } from "../lib/chat";
import { InfoTip } from "./InfoTip";
import "./ContextMeter.css";
import { compactTokens } from "../lib/usage";
import { matches } from "../lib/shortcuts";
import type { AsideTab } from "../types";

// How full the model's window is, as a ring beside the send button; a click opens
// what the tokens are spent on and the one thing to do about it — fold the
// older part of the conversation into a summary.
//
// The parts are shown apart because they behave differently: folding moves
// the conversation and leaves the rest exactly where it was, and skills and
// MCP servers are the user's to switch off — each row is something to do.
// This is the backend's estimate; the provider's own count for the last
// request goes next to it rather than implying the two agree.

// Zero is also what a provider that reports nothing sends, so it is not
// called a miss.
const cacheNote = ({ promptTokens, cachedTokens = 0 }: ChatUsage) =>
  cachedTokens && promptTokens
    ? `, ${compactTokens(cachedTokens)} of it (${Math.round((cachedTokens / promptTokens) * 100)}%) from the cache`
    : ", none of it reported as cached";

// Of the last request; a dash where nothing was reported as cached, for the same reason.
const cacheShare = ({ promptTokens, cachedTokens = 0 }: ChatUsage) =>
  cachedTokens && promptTokens ? `${Math.round((cachedTokens / promptTokens) * 100)}%` : "—";

// In the order of the bar: the fixed parts first, the conversation last.
const PARTS = [
  { key: "instructions", label: "Instructions and rules" },
  { key: "tools", label: "Built-in tools" },
  { key: "skills", label: "Skills" },
  { key: "mcp", label: "MCP servers" },
  { key: "conversation", label: "Conversation" },
] as const;

/** The parts something can be done about: the pane each is made in. */
const PANE_OF: Partial<Record<(typeof PARTS)[number]["key"], AsideTab>> = { instructions: "rules", skills: "skills", mcp: "mcp" };

const R = 7;
const CIRCUMFERENCE = 2 * Math.PI * R;

// No window is a number with no scale — the ring stays empty rather than
// filling against a guess.
const shareOf = (context: ContextUsage) => (context.limit ? Math.min(1, context.total / context.limit) : null);

/** What the tokens are spent on: the ring's panel, and the Overview pane's. `cache` adds the
    last request's cache hit as a row (the Overview's — the ring says it in a sentence); `children`
    go under the rows. `spent` adds what the whole session has cost, every model call of it;
    `subagents` the share of it the chat's subagents spent, under it.
    `onOpenPane` makes a part's row a link to the pane it is made in. A `null` context is one
    still being read: the same rows with placeholders where the numbers go, so nothing moves
    when they arrive. */
export function ContextDetails({
  context,
  cache,
  spent,
  subagents,
  onOpenPane,
  children,
}: {
  context: ContextUsage | null;
  cache?: ChatUsage | null;
  spent?: number | null;
  subagents?: number | null;
  onOpenPane?: (pane: AsideTab) => void;
  children?: ReactNode;
}) {
  const share = context && shareOf(context);
  const percent = share == null ? null : Math.round(share * 100);
  const reading = <span className="ctx-skeleton" />;
  return (
    <>
      <div className="ctx-head">
        <span>Context window</span>
        {percent !== null && <span className="ctx-percent">{percent}%</span>}
      </div>

      {!context ? (
        <>
          <div className="ctx-bar" aria-hidden="true" />
          <p className="ctx-total">{reading}</p>
        </>
      ) : context.limit ? (
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
            {compactTokens(context.total)} of {compactTokens(context.limit)} tokens
          </p>
        </>
      ) : (
        <p className="ctx-total">
          About {compactTokens(context.total)} tokens in the next request. The model's window is not known —
          set it in Settings → Models.
        </p>
      )}

      <dl className="ctx-rows">
        {PARTS.map(({ key, label }) => {
          const pane = PANE_OF[key];
          return (
            <div key={key}>
              <dt>
                <i className={`ctx-dot ctx-part--${key}`} />
                {pane && onOpenPane ? (
                  <button type="button" className="ctx-link" onClick={() => onOpenPane(pane)}>
                    {label}
                  </button>
                ) : (
                  label
                )}
              </dt>
              <dd>{context ? compactTokens(context[key]) : reading}</dd>
            </div>
          );
        })}
        {cache !== undefined && (
          <div>
            <dt>
              <i className="ctx-dot ctx-dot--cache" />
              Cache hit
            </dt>
            <dd>{cache ? cacheShare(cache) : context ? "—" : reading}</dd>
          </div>
        )}
        {spent !== undefined && (
          <div title="Every model call of this chat: its turns, helpers, title, checks and summaries">
            <dt>
              <i className="ctx-dot ctx-dot--spent" />
              Session tokens
            </dt>
            <dd>{spent !== null ? compactTokens(spent) : context ? "—" : reading}</dd>
          </div>
        )}
        {subagents !== undefined && (
          <div>
            <dt>
              <i className="ctx-dot ctx-dot--subagents" />
              Subagents tokens
              <InfoTip text="Only this chat's subagents' model calls. Session tokens above count every call of the chat — these included." />
            </dt>
            <dd>{subagents !== null ? compactTokens(subagents) : context ? "—" : reading}</dd>
          </div>
        )}
      </dl>

      {children}
    </>
  );
}

type Props = {
  context: ContextUsage;
  usage: ChatUsage | null;
  /** Mid-turn the history is the turn's: folding it from under a running request is not offered. */
  running: boolean;
  onCompact: () => void;
  /** Opens the panel upwards — for a meter at the bottom of the window. */
  up?: boolean;
  /** The session's total, as a row — Chat mode's; the agent's is on the Overview. */
  spent?: number | null;
};

export function ContextMeter({ context, usage, running, onCompact, up = false, spent }: Props) {
  const [open, setOpen] = useState(false);
  const wrap = useRef<HTMLDivElement>(null);
  const pop = useRef<HTMLDivElement>(null);
  // The panel lives in the body, not in the column: a column clips what
  // overflows it, and a narrow one has no room for a panel anchored to the
  // ring's edge. Its place is measured from the ring and kept inside the
  // window — the side with more room wins, and the panel scrolls within it.
  const [place, setPlace] = useState<CSSProperties | null>(null);

  useLayoutEffect(() => {
    if (!open) {
      setPlace(null);
      return;
    }
    const fit = () => {
      const el = wrap.current;
      if (!el) return;
      const r = el.getBoundingClientRect();
      const vw = window.innerWidth;
      const vh = window.innerHeight;
      const width = Math.min(280, vw - 16);
      const left = Math.max(8, Math.min(r.right - width, vw - width - 8));
      const above = r.top - 12;
      const below = vh - r.bottom - 12;
      const useUp = up ? above >= below : below >= above;
      setPlace({
        position: "fixed",
        left,
        width,
        right: "auto",
        maxHeight: Math.max(120, (useUp ? above : below) - 8),
        overflowY: "auto",
        ...(useUp ? { bottom: vh - r.top + 6, top: "auto" } : { top: r.bottom + 6, bottom: "auto" }),
      });
    };
    fit();
    window.addEventListener("resize", fit);
    return () => window.removeEventListener("resize", fit);
  }, [open, up]);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: PointerEvent) => {
      const t = e.target as Node;
      if (!wrap.current?.contains(t) && !pop.current?.contains(t)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => matches(e, "close") && setOpen(false);
    document.addEventListener("pointerdown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("pointerdown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  const share = shareOf(context);
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

      {open &&
        createPortal(
        <div ref={pop} className="ctx-pop" style={place ?? undefined} role="dialog" aria-label="Context">
          <ContextDetails context={context} spent={spent}>
            {context.compactsAt && (
              <p className="ctx-note">Folds the older part on its own at {compactTokens(context.compactsAt)}.</p>
            )}
            {usage && (
              <p className="ctx-note">
                The last request actually cost {compactTokens(usage.promptTokens)}
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
          </ContextDetails>
        </div>,
        document.body,
      )}
    </div>
  );
}
