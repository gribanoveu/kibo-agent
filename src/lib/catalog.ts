import { invoke } from "@tauri-apps/api/core";

/** What a catalog entry installs as (docs/32-catalog.md). */
export type CatalogKind = "skill" | "command" | "runbook" | "mcp" | "hooks";

export type CatalogItem = {
  id: string;
  kind: CatalogKind;
  name: string;
  description: string;
  rev: string;
  /** A skill's folder, every other kind's one file — relative to the source. */
  path: string;
  /** A skill's files, relative to `path`. */
  files: string[];
  role?: string;
};

/** One catalog as the page shows it. `error` beside `items`: the items are the copy from `fetchedAt`. */
export type CatalogSource = {
  url: string;
  builtIn: boolean;
  items: CatalogItem[];
  /** Index entries left out, each with why. */
  problems: string[];
  error: string | null;
  /** Unix seconds of the copy shown. */
  fetchedAt: number | null;
};

/** Every catalog asked for its list — a conditional GET each, a copy kept for when there is no network. */
export async function catalogList(): Promise<CatalogSource[]> {
  return invoke<CatalogSource[]>("catalog_list");
}
