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
  /** The rev this source installed, when it did: one other than `rev` can be updated. */
  installedRev: string | null;
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

/** One file of an update: what is there now beside what the catalog has now. */
export type CatalogFileChange = { name: string; /** Unified; empty when the file is the same. */ diff: string; added: number; removed: number };

export type CatalogUpdatePreview = {
  changes: CatalogFileChange[];
  /** A server's placeholders the installed entry does not give back a value for. */
  secrets: string[];
  /** Changed here since it was installed. */
  edited: boolean;
};

/** What updating an entry `source` installed would change. */
export async function catalogUpdatePreview(source: string, kind: CatalogKind, id: string): Promise<CatalogUpdatePreview> {
  return invoke<CatalogUpdatePreview>("catalog_update_preview", { source, kind, id });
}

/** Writes the catalog's new version over what `source` installed; a server keeps its secrets unless `secrets` gives them again. */
export async function catalogUpdate(source: string, kind: CatalogKind, id: string, secrets: Record<string, string> = {}): Promise<void> {
  return invoke("catalog_update", { source, kind, id, secrets });
}

export async function catalogPreview(source: string, kind: CatalogKind, id: string): Promise<CatalogPreview> {
  return invoke<CatalogPreview>("catalog_preview", { source, kind, id });
}
