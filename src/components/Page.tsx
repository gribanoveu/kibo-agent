import type { ComponentType, ReactElement, ReactNode } from "react";
import { X } from "lucide-react";
import "./Page.css";

type PageProps = {
  title: string;
  /** Under the title — tabs, a search, buttons — and pinned with it while the page scrolls. */
  bar?: ReactNode;
  /** A `PagePanel` right of the page while one is open. */
  panel?: ReactNode;
  /** The page's `PageContext.sideHandle`: it goes between the page and its panel. */
  sideHandle: ReactElement;
  children: ReactNode;
};

/**
 * A sidebar page (pages.tsx): a title and a bar pinned at the top, what the
 * page lists under them, and an optional panel on the right — where the panes
 * sit beside the chat, as wide as they are, and dragged by the same handle.
 * Every page is drawn with it, so a new one only fills it in.
 */
export function Page({ title, bar, panel, sideHandle, children }: PageProps) {
  return (
    <>
      <section className="page" aria-label={title}>
        <header className="page-top">
          <h1 className="page-title">{title}</h1>
          {bar && <div className="page-bar">{bar}</div>}
        </header>
        <div className="page-body">{children}</div>
      </section>
      {panel && sideHandle}
      {panel}
    </>
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

/** The panel right of a page, in the look of the panes beside the chat (AsidePanel): a heading with a close button, and a body that scrolls on its own. */
export function PagePanel({ icon: Icon, title, label, onClose, children }: PanelProps) {
  return (
    <aside className="page-panel" aria-label={label}>
      <div className="page-panel-head">
        <h2>
          <Icon size={14} />
          {title}
        </h2>
        <button type="button" className="page-panel-close" title="Close panel" aria-label="Close panel" onClick={onClose}>
          <X size={14} />
        </button>
      </div>
      <div className="page-panel-body">{children}</div>
    </aside>
  );
}
