import { useCallback, useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { ChevronLeft, ChevronRight, MoreVertical } from "lucide-react";
import "./ChatMenu.css";
import { matches } from "../lib/shortcuts";

/**
 * One thing the menu can do to the open chat. `hint` is the line under the
 * label — the place to say why an item is disabled, which is the question a
 * greyed-out row always raises.
 */
export type ChatMenuItem = {
  id: string;
  label: string;
  hint?: string;
  /** The key that does the same, drawn at the row's end. */
  shortcut?: string;
  icon?: ReactNode;
  disabled?: boolean;
  /** Starts a new group: a line above this row. */
  divided?: boolean;
  /** A second level — "To folder" and its folders: the row opens these
      instead of acting, with a back row above them. */
  submenu?: ChatMenuItem[];
  onSelect: () => void;
};

/**
 * The chat header's "⋮" — actions on the conversation itself, kept out of the
 * header so it stays a title and a branch.
 *
 * A list rather than one button because that is what it is for: exporting is
 * the first of these, and the next one is a row in `items`. The sidebar's
 * rows use it too, for archiving and deleting a chat.
 *
 * The open menu is portaled to the viewport: inside a row it would be
 * clipped by the scrollable list's edges and carried away on scroll. Fixed
 * at the trigger, it flips above when the window's lower edge is nearer,
 * and scrolling or resizing closes it — the trigger it names has moved.
 */
export function ChatMenu({ items }: { items: ChatMenuItem[] }) {
  const [open, setOpen] = useState(false);
  // The levels drilled into: the last is what the menu shows. Picking from a
  // submenu acts and closes, like picking from the top level.
  const [trail, setTrail] = useState<ChatMenuItem[][]>([]);
  const wrap = useRef<HTMLDivElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ top: number; left: number } | null>(null);

  const close = useCallback(() => {
    setOpen(false);
    setTrail([]);
    setPos(null);
  }, []);

  // Place after every render while open: below the trigger, flipped above
  // when the menu would pass the window's lower edge, never past its right.
  useLayoutEffect(() => {
    if (!open) return;
    const button = wrap.current?.getBoundingClientRect();
    const box = menu.current?.getBoundingClientRect();
    if (!button || !box) return;
    const below = button.bottom + 6;
    const top = below + box.height > window.innerHeight - 8 && button.top - 6 - box.height >= 8 ? button.top - 6 - box.height : below;
    const left = Math.max(8, Math.min(button.right - box.width, window.innerWidth - box.width - 8));
    setPos((was) => (was && Math.abs(was.top - top) < 0.5 && Math.abs(was.left - left) < 0.5 ? was : { top, left }));
  });

  // Dismissed the way every other menu in the app is — an outside press or
  // Escape. The press reaches into the portaled menu as well as the trigger.
  useEffect(() => {
    if (!open) return;
    const onDown = (e: PointerEvent) => {
      if (!wrap.current?.contains(e.target as Node) && !menu.current?.contains(e.target as Node)) close();
    };
    const onKey = (e: KeyboardEvent) => {
      if (!matches(e, "close")) return;
      e.preventDefault();
      if (trail.length > 0) {
        setTrail((was) => was.slice(0, -1));
      } else {
        close();
      }
    };
    document.addEventListener("pointerdown", onDown);
    document.addEventListener("keydown", onKey, true);
    return () => {
      document.removeEventListener("pointerdown", onDown);
      document.removeEventListener("keydown", onKey, true);
    };
  }, [open, trail.length, close]);

  // The trigger moves with the scrollable list under the fixed menu: rather
  // than point the menu at another row, it goes away.
  useEffect(() => {
    if (!open) return;
    document.addEventListener("scroll", close, true);
    window.addEventListener("resize", close);
    return () => {
      document.removeEventListener("scroll", close, true);
      window.removeEventListener("resize", close);
    };
  }, [open, close]);

  const shown = trail.length > 0 ? trail[trail.length - 1] : items;

  return (
    <div className="chat-menu-wrap" ref={wrap}>
      <button
        type="button"
        className={`iconbtn chat-menu-button${open ? " on" : ""}`}
        title="More"
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => {
          if (open) {
            close();
            return;
          }
          const button = wrap.current?.getBoundingClientRect();
          setPos(button ? { top: button.bottom + 6, left: button.right - 190 } : null);
          setOpen(true);
        }}
      >
        <MoreVertical size={15} />
      </button>
      {open &&
        pos &&
        createPortal(
          <div ref={menu} className="chat-menu" role="menu" style={{ position: "fixed", top: pos.top, left: pos.left, right: "auto" }}>
            {trail.length > 0 && (
              <button type="button" role="menuitem" className="chat-menu-item chat-menu-back" onClick={() => setTrail((was) => was.slice(0, -1))}>
                <span className="chat-menu-icon">
                  <ChevronLeft size={14} />
                </span>
                <span className="chat-menu-text">
                  <span>Back</span>
                </span>
              </button>
            )}
            {shown.map((item) => (
              <button
                key={item.id}
                type="button"
                role="menuitem"
                className={`chat-menu-item${item.divided ? " divided" : ""}`}
                disabled={item.disabled}
                onClick={() => {
                  if (item.submenu) {
                    setTrail((was) => [...was, item.submenu!]);
                    return;
                  }
                  close();
                  item.onSelect();
                }}
              >
                {item.icon && <span className="chat-menu-icon">{item.icon}</span>}
                <span className="chat-menu-text">
                  <span>{item.label}</span>
                  {item.hint && <span className="hint">{item.hint}</span>}
                </span>
                {item.shortcut && <kbd className="chat-menu-shortcut" aria-hidden>{item.shortcut}</kbd>}
                {item.submenu && <ChevronRight className="chat-menu-sub" size={13} aria-hidden />}
              </button>
            ))}
          </div>,
          document.body,
        )}
    </div>
  );
}
