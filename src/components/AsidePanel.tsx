import { useEffect, useState } from "react";
import { ArrowLeft, X } from "lucide-react";
import { PANES, type Dock, type PaneContext } from "./panes";
import type { AsideTab } from "../types";
import "./AsidePanel.css";

type Props = {
  /** Which pane is showing. It is picked from the chat header's "⋮". */
  tab: AsideTab;
  dock: Dock;
  /** What the panes are drawn from; `active` is whether this dock is on screen. */
  ctx: PaneContext;
  onClose: () => void;
};

/** One dock of the window: a heading naming its pane, and the pane — only the
    pane, for one the chat header has its own button for. */
export function AsidePanel({ tab, dock, ctx, onClose }: Props) {
  const pane = PANES.find((p) => p.id === tab);
  // The pane Overview led to, while it is the one showing: it goes back instead of closing.
  const [ledTo, setLedTo] = useState<AsideTab | null>(null);
  useEffect(() => {
    if (tab !== ledTo) setLedTo(null);
  }, [tab]);
  const overview = {
    ...ctx.overview,
    onOpenPanel: (next: AsideTab) => {
      setLedTo(next);
      ctx.overview.onOpenPanel(next);
    },
  };
  return (
    // Keyed by the pane: one taking another's place slides in as an opened one does.
    <aside key={tab} className={`aside aside-${dock}`}>
      {!pane?.headless && (
        <div className="aside-head">
          {pane && (
            <h2 className="aside-title">
              <pane.icon size={14} />
              {pane.label}
            </h2>
          )}
          {tab === ledTo ? (
            <button type="button" className="iconbtn aside-close" title="Back to Overview" onClick={() => ctx.overview.onOpenPanel("overview")}>
              <ArrowLeft size={14} />
            </button>
          ) : (
            <button type="button" className="iconbtn aside-close" title="Close panel" onClick={onClose}>
              <X size={14} />
            </button>
          )}
        </div>
      )}

      <div className="aside-body">
        <div className="tabpanel">{pane && <pane.Component {...ctx} overview={overview} onClosePane={onClose} />}</div>
      </div>
    </aside>
  );
}
