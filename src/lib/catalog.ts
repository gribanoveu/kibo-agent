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
  /** Where it goes (`~/.kibo/skills/review`, a server's `~/.kibo/mcp.json`, hooks' `~/.kibo/hooks.json`); null for an entry with no place. */
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

/** Installs an entry as the source's kept index lists it; a server with its `${NAME}`s filled from `secrets`. */
export async function catalogInstall(source: string, kind: CatalogKind, id: string, secrets: Record<string, string> = {}): Promise<void> {
  return invoke("catalog_install", { source, kind, id, secrets });
}

/** Removes what `source` installed as `kind`/`id`. */
export async function catalogRemove(source: string, kind: CatalogKind, id: string): Promise<void> {
  return invoke("catalog_remove", { source, kind, id });
}

/** An entry as its source has it. */
export type CatalogPreview = {
  /** A skill's SKILL.md, any other kind's one file. */
  text: string;
  /** A server's `${NAME}` placeholders: installing it asks for each. */
  secrets: string[];
};

export async function catalogPreview(source: string, kind: CatalogKind, id: string): Promise<CatalogPreview> {
  return invoke<CatalogPreview>("catalog_preview", { source, kind, id });
}
