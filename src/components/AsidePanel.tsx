import { X } from "lucide-react";
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
  return (
    <aside className={`aside aside-${dock}`}>
      {!pane?.headless && (
        <div className="aside-head">
          {pane && (
            <h2 className="aside-title">
              <pane.icon size={14} />
              {pane.label}
            </h2>
          )}
          <button type="button" className="iconbtn aside-close" title="Close panel" onClick={onClose}>
            <X size={14} />
          </button>
        </div>
      )}

      <div className="aside-body">
        <div className="tabpanel">{pane && <pane.Component {...ctx} />}</div>
      </div>
    </aside>
  );
}
