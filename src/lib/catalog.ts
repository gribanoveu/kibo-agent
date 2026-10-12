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

/** What a row says about an entry: nothing in its place, installed from here
    (maybe with a newer rev listed, maybe edited since), or something else in its place. */
export type CatalogState = "available" | "installed" | "update" | "changed" | "taken";

/** An entry with what this machine has of it. */
export type CatalogEntry = CatalogItem & {
  state: CatalogState;
  /** Where it goes (`~/.kibo/skills/review`); null for a kind not installed as files yet. */
  target: string | null;
  /** A runbook named as one the app ships: installed, it is read instead. */
  replacesBuiltIn: boolean;
};

/** One catalog as the page shows it. `error` beside `items`: the items are the copy from `fetchedAt`. */
export type CatalogSource = {
  url: string;
  builtIn: boolean;
  items: CatalogEntry[];
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

/** Installs a skill, command or runbook as the source's kept index lists it. */
export async function catalogInstall(source: string, kind: CatalogKind, id: string): Promise<void> {
  return invoke("catalog_install", { source, kind, id });
}

/** Removes what `source` installed as `kind`/`id`. */
export async function catalogRemove(source: string, kind: CatalogKind, id: string): Promise<void> {
  return invoke("catalog_remove", { source, kind, id });
}

/** An entry's text from its source: a skill's SKILL.md, any other kind's one file. */
export async function catalogPreview(source: string, kind: CatalogKind, id: string): Promise<string> {
  return invoke<string>("catalog_preview", { source, kind, id });
}
