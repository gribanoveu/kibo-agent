import { useEffect, type ComponentType, type ReactElement, type ReactNode } from "react";
import { X } from "lucide-react";
import { matches } from "../lib/shortcuts";
import "./Page.css";

type PageProps = {
  title: string;
  /** Under the title — tabs, a search, buttons — and pinned with it while the page scrolls. */
  bar?: ReactNode;
  /** A `PagePanel` right of the page while one is open. */
  panel?: ReactNode;
  /** A `PageSheet` over the page while one is open: it moves nothing aside. */
  sheet?: ReactNode;
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
export function Page({ title, bar, panel, sheet, sideHandle, children }: PageProps) {
  return (
    <>
      <section className="page" aria-label={title}>
        <header className="page-top">
          <h1 className="page-title">{title}</h1>
          {bar && <div className="page-bar">{bar}</div>}
        </header>
        <div className="page-body">{children}</div>
        {sheet}
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
export function PagePanel(props: PanelProps) {
  return <PanelBox className="page-panel" {...props} />;
}

/**
 * The same panel laid over the page's right side instead of beside it: the
 * page keeps its width, and what shows of it left of the sheet still answers
 * a click — another item opens in the same sheet. Escape closes it, unless a
 * menu or a dialog is open over it: that layer closes first.
 */
export function PageSheet(props: PanelProps) {
  const { onClose } = props;
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => matches(e, "close") && !document.querySelector(".modal-backdrop, .dropdown-menu") && onClose();
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose]);
  return <PanelBox className="page-sheet" {...props} />;
}

function PanelBox({ className, icon: Icon, title, label, onClose, children }: PanelProps & { className: string }) {
  return (
    <aside className={className} aria-label={label}>
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
