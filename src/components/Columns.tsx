import type { ReactElement } from "react";
import { PanelResizeHandle } from "./PanelResizeHandle";
import type { usePanelSizes } from "../hooks/usePanelSizes";
import "./Columns.css";

type Props = {
  /** Files left, the file in the middle, the chat right — or the chat first, the rest right of it. */
  docs: boolean;
  chat: ReactElement;
  /** The file viewer, when a file is open. */
  viewer: ReactElement | null;
  /** The pane from the header's button; hidden by the window's class rather than unmounted. */
  top: ReactElement;
  topShown: boolean;
  /** The bottom dock: under the top pane, or under the file in the docs layout. */
  bottom: ReactElement | null;
  panels: ReturnType<typeof usePanelSizes>;
};

/**
 * The window's columns right of the sidebar, in the order the layout puts
 * them, with a resize handle between each two. One keyed list in either
 * layout: switching moves the chat rather than drawing it anew, so a draft
 * survives it.
 */
export function Columns({ docs, chat, viewer, top, topShown, bottom, panels }: Props) {
  const bottomHandle = (
    <PanelResizeHandle
      axis="y"
      invert
      ariaLabel="Resize the bottom panel"
      onResize={panels.resizeBottomBy}
      onResizeEnd={(size) => panels.endResize("bottom", size)}
    />
  );
  // The dock's handle sizes the column beside it: right of the dock in the
  // docs layout, left of it otherwise.
  const dockHandle = (
    <PanelResizeHandle
      key="dock-handle"
      invert={!docs}
      ariaLabel="Resize the side panel"
      onResize={panels.resizeAsideBy}
      onResizeEnd={(size) => panels.endResize("aside", size)}
    />
  );
  if (docs)
    return [
      <div key="dock" className="dock-column">
        {top}
      </div>,
      topShown && dockHandle,
      <div key="middle" className="docs-middle">
        {viewer ?? <div className="docs-empty">Pick a file in the tree to read it here.</div>}
        {bottom && bottomHandle}
        {bottom}
      </div>,
      <PanelResizeHandle
        key="chat-handle"
        invert
        ariaLabel="Resize the chat"
        onResize={panels.resizeChatBy}
        onResizeEnd={(size) => panels.endResize("chat", size)}
      />,
      chat,
    ];
  return [
    chat,
    viewer && (
      <PanelResizeHandle
        key="viewer-handle"
        invert
        ariaLabel="Resize the file viewer"
        onResize={panels.resizeViewerBy}
        onResizeEnd={(size) => panels.endResize("viewer", size)}
      />
    ),
    viewer,
    (topShown || bottom) && dockHandle,
    // Either may be closed; the column goes when both are.
    <div key="dock" className="dock-column">
      {top}
      {bottom && topShown && bottomHandle}
      {bottom}
    </div>,
  ];
}
