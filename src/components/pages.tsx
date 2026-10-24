import type { ReactElement, ReactNode } from "react";
import { AudioLines, FolderOpen, Library, SquareKanban, Store } from "lucide-react";
import { ArtifactsPage } from "./ArtifactsPage";
import { BoardPage } from "./BoardPage";
import { CatalogPage } from "./CatalogPage";
import { KnowledgePage } from "./KnowledgePage";
import { MeetingsPage } from "./MeetingsPage";
import type { AsideTab } from "../types";
import "./pages.css";

/** What a page gets from the window. */
export type PageContext = {
  /** Shows a pane in the column beside the page — only a page with `dock` has one. */
  openPane: (id: AsideTab) => void;
  /** The handle to put left of a panel the page shows on its right: it sizes
      that panel as the dock's handle sizes the dock, to the same width. */
  sideHandle: ReactElement;
  /** What the page was opened on — a meeting's id for Meetings. */
  item?: string;
  /** Another page, opened on `item`. */
  openPage: (id: PageId, item?: string) => void;
  /** A Chat mode chat opened, leaving the page — a prototype's. */
  openChat: (id: string) => void;
  /** A Chat mode chat deleted, as the sidebar deletes one; `false` while it must stay open. */
  removeChat: (id: string) => Promise<boolean>;
  /** The Chat mode chats, as the sidebar lists them. */
  chats: readonly { id: string }[];
};

export type PageDef = {
  id: string;
  label: string;
  /** The row's tooltip, read on the rail where the label is hidden. */
  title: string;
  icon: typeof Store;
  /** The panes' column stays beside the page (MCP, Skills, Terminal…), in either mode;
      without it the page takes everything right of the sidebar. */
  dock: boolean;
  /** The feature it belongs to (`lib/features.ts`): listed only while that is on. */
  feature?: string;
  Component: (ctx: PageContext) => ReactNode;
};

// Every page the sidebar lists under the modes, in its order. A page is drawn
// with `Page` (title, pinned bar, its content) and opens what it is about in a
// `PagePanel` on its right (Page.tsx). It takes the chat's place, and the
// sidebar drops the chat list while it is open; the chat stays mounted
// underneath, so a draft or a running turn survives a visit. Adding one is an
// entry here — nothing else names a page.
export const PAGES = [
  { id: "catalog", label: "Catalog", title: "Catalog — skills, commands, MCP servers and hooks to install", icon: Store, dock: false, Component: CatalogPage },
  { id: "board", label: "Board", title: "Board — what the Assistant keeps for later", icon: SquareKanban, dock: false, feature: "board", Component: BoardPage },
  { id: "meetings", label: "Meetings", title: "Meetings — a call's recording made into notes", icon: AudioLines, dock: false, feature: "meeting-notes", Component: MeetingsPage },
  { id: "knowledge", label: "Knowledge", title: "Knowledge — what agents wrote about repositories", icon: Library, dock: false, feature: "knowledge", Component: KnowledgePage },
  { id: "artifacts", label: "Artifacts", title: "Artifacts — meetings and prototypes, as folders", icon: FolderOpen, dock: false, Component: ArtifactsPage },
] as const satisfies readonly PageDef[];

export type PageId = (typeof PAGES)[number]["id"];

/** The pages the sidebar lists: every one not of a feature, and those of the features that are on. */
export const pagesShown = (isOn: (feature: string) => boolean) =>
  PAGES.filter((page) => {
    const feature = (page as PageDef).feature;
    return !feature || isOn(feature);
  });
