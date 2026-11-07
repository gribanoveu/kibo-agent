import { useEffect, useRef, type ComponentType, type ReactNode } from "react";
import { X } from "lucide-react";
import { matches } from "../lib/shortcuts";
import "./Page.css";

type PageProps = {
  title: string;
  /** Tabs, a search, buttons — pinned while the page scrolls. */
  bar?: ReactNode;
  /** A `PageSheet` over the page while one is open: it moves nothing aside. */
  sheet?: ReactNode;
  children: ReactNode;
};

/**
 * A sidebar page (pages.tsx): a bar pinned at the top, what the page lists under
 * it, and the sheet of what it opened over its right side. The name is in the
 * window's title bar (App.tsx); `title` is what a screen reader calls the page.
 * Every page is drawn with it, so a new one only fills it in.
 */
export function Page({ title, bar, sheet, children }: PageProps) {
  return (
    <section className="page" aria-label={title}>
      {bar && <header className="page-bar">{bar}</header>}
      <div className="page-body">{children}</div>
      {sheet}
    </section>
  );
}

type PanelProps = {
  icon: ComponentType<{ size?: number }>;
  /** The heading's word, as a pane names itself ("Skill", "MCP"). */
  title: string;
  /** What a screen reader calls the panel — the thing it is about. */
  label: string;
  onClose: () => void;
  children: ReactNode;
};

/** What a page says while it has nothing to show; `action` is what starts it. */
export function PageEmpty({ icon: Icon, action, children }: { icon: ComponentType<{ size?: number }>; action?: ReactNode; children: ReactNode }) {
  return (
    <div className="page-empty">
      <Icon size={24} />
      <p>{children}</p>
      {action}
    </div>
  );
}

/**
 * What a page opens, laid over its right side in the look of the panes beside
 * the chat (AsidePanel): a heading with a close button, and a body that
 * scrolls on its own. The page keeps its width, and what shows of it left of
 * the sheet still answers a click — another item opens in the same sheet.
 * Escape or a click anywhere else closes it, unless a menu or a dialog is
 * open over it — that layer closes first. A key that went to a text field is
 * the field's; a click on what opens things in the sheet (`data-sheet-opener`:
 * a card, a row, a toggle) is that one's to handle.
 */
export function PageSheet({ icon: Icon, title, label, onClose, children }: PanelProps) {
  const box = useRef<HTMLElement>(null);
  useEffect(() => {
    const layered = () => document.querySelector('.dropdown-menu, [role="dialog"]') !== null;
    const onKey = (e: KeyboardEvent) => {
      const typing = e.target instanceof Element && e.target.closest("input, textarea, [contenteditable]");
      if (matches(e, "close") && !typing && !layered()) onClose();
    };
    const onDown = (e: PointerEvent) => {
      const at = e.target instanceof Element ? e.target : null;
      if (at && (box.current?.contains(at) || at.closest("[data-sheet-opener]"))) return;
      if (!layered()) onClose();
    };
    document.addEventListener("keydown", onKey);
    document.addEventListener("pointerdown", onDown);
    return () => {
      document.removeEventListener("keydown", onKey);
      document.removeEventListener("pointerdown", onDown);
    };
  }, [onClose]);
  return (
    <aside ref={box} className="page-sheet" aria-label={label}>
      <div className="page-sheet-head">
        <h2>
          <Icon size={14} />
          {title}
        </h2>
        <button type="button" className="page-sheet-close" title="Close panel" aria-label="Close panel" onClick={onClose}>
          <X size={14} />
        </button>
      </div>
      {/* `text-scale`: what a sheet holds reads at the user's text size, as the chat does. */}
      <div className="page-sheet-body text-scale">{children}</div>
    </aside>
  );
}
