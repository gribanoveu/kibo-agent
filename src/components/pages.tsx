import type { ReactNode } from "react";
import { Store } from "lucide-react";
import { CatalogPage } from "./CatalogPage";
import type { AsideTab } from "../types";
import "./pages.css";

/** What a page gets from the window. */
export type PageContext = {
  /** Shows a pane in the column beside the page — only a page with `dock` has one. */
  openPane: (id: AsideTab) => void;
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
  Component: (ctx: PageContext) => ReactNode;
};

// Every page the sidebar lists under the modes, in its order. A page takes the
// chat's place, and the sidebar drops the chat list while it is open; the chat stays mounted underneath, so a draft or a running turn
// survives a visit. Adding one is an entry here — nothing else names a page.
export const PAGES = [
  { id: "catalog", label: "Catalog", title: "Catalog — skills, commands, MCP servers and hooks to install", icon: Store, dock: false, Component: CatalogPage },
] as const satisfies readonly PageDef[];

export type PageId = (typeof PAGES)[number]["id"];
