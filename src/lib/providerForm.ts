// Pure pieces of the provider form: which models a search leaves, the known
// providers, and what a certificate file holds.

import type { ProviderKind } from "./settings";
import catalog from "./providerCatalog.json";

/**
 * The models a search leaves, in the order given. Every word has to appear
 * somewhere in the name, in any order and any case — "sonnet 4" finds
 * `claude-sonnet-4-5`.
 */
export function filterModels(models: string[], query: string): string[] {
  return filterBy(models, query, (model) => model);
}

/** What `filterModels` does, for anything that has a text to search. */
export function filterBy<T>(items: T[], query: string, text: (item: T) => string): T[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (!words.length) return items;
  return items.filter((item) => {
    const name = text(item).toLowerCase();
    return words.every((word) => name.includes(word));
  });
}

export type CatalogEntry = { id: string; name: string; kind: ProviderKind; baseUrl: string };

/**
 * Providers the form can fill in from a list instead of a typed URL: a
 * snapshot of models.dev, the list OpenCode uses, rebuilt by
 * `scripts/update-provider-catalog.py`.
 */
export const CATALOG = catalog as CatalogEntry[];

/**
 * `id`, or `id-2`, `id-3`… when a provider is already saved under it: saving
 * under a taken name replaces that provider.
 */
export function freeName(id: string, taken: string[]): string {
  let name = id;
  for (let n = 2; taken.includes(name); n++) name = `${id}-${n}`;
  return name;
}

/** How many certificates a PEM holds — what the form shows instead of the text. */
export function certificateCount(pem: string): number {
  return pem.match(/-----BEGIN CERTIFICATE-----/g)?.length ?? 0;
}

/**
 * A certificate file's contents as PEM. A `.cer` or `.der` is commonly the
 * binary encoding, which the backend does not read, so it is wrapped here;
 * text is taken as it is.
 */
export function pemFromBytes(bytes: Uint8Array): string {
  const text = new TextDecoder().decode(bytes);
  if (text.includes("-----BEGIN")) return text.trim();
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  const base64 = btoa(binary).replace(/.{64}/g, "$&\n").trim();
  return `-----BEGIN CERTIFICATE-----\n${base64}\n-----END CERTIFICATE-----`;
}

/**
 * The thinking levels every reasoning API takes — OpenAI's `reasoning_effort`
 * and Anthropic's adaptive `effort` both know these three. "" is the model's
 * own default: nothing is sent. Anything else (`max`, `minimal`, a token
 * budget) is still typed by hand in Settings.
 */
export const EFFORTS: { value: string; label: string }[] = [
  { value: "", label: "Default" },
  { value: "low", label: "Low" },
  { value: "medium", label: "Medium" },
  { value: "high", label: "High" },
];

/** The levels to offer, with `current` added when it is one typed by hand — so it still shows as chosen. */
export function effortOptions(current: string): { value: string; label: string }[] {
  return EFFORTS.some((e) => e.value === current) ? EFFORTS : [...EFFORTS, { value: current, label: current }];
}
