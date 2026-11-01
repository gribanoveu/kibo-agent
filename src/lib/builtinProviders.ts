// The providers the app carries itself, offered ahead of the catalog generated
// from models.dev: the handful nearly everyone configures, and a gateway that
// needs more than a name and a URL — a certificate, headers — which the
// generated list has no field for.

import type { CatalogEntry } from "./providerForm";
import { CATALOG } from "./providerForm";
import type { ProviderConfig } from "./settings";
import alfagenCertificate from "./certs/alfa-bank-st-ca-root.pem?raw";

/**
 * A known provider that comes with the app rather than from models.dev.
 *
 * `trustedCertPem` replaces the public roots for that provider alone — the
 * same field the form's Certificate card fills by hand — and `requestHeaders`
 * is what a gateway of its own wants on every request, which the form has no
 * fields for. A row that carries neither is here for its place in the list.
 */
export type BuiltinProvider = CatalogEntry & Pick<ProviderConfig, "trustedCertPem" | "requestHeaders">;

/**
 * AlfaGen, the bank's own gateway. Its certificate is the `Alfa-Bank ST CA
 * Root` the gateway's chain leads to; the intermediate is not bundled because
 * the server sends it, and the leaf on it would be pinned to a certificate
 * that expires first (docs/47-alfagen-provider.md, decision 3).
 */
export const BUILTIN_PROVIDERS: BuiltinProvider[] = [
  {
    id: "alfagen",
    name: "AlfaGen",
    kind: "openAiCompatible",
    baseUrl: "https://alfagen.moscow.alfaintra.net/continue-dev/v1",
    trustedCertPem: alfagenCertificate.trim(),
    // The gateway tells its clients apart by this line. The placeholder is
    // sent as it is and expanded to the build's version on each request, so an
    // update does not leave a stale version in the settings. Its spelling is a
    // contract with `REQUEST_HEADER_VALUE_VERSION` in
    // `infra/llm_providers/openai_compatible.rs` — a placeholder the backend does
    // not know goes out literally, and nothing in the app complains.
    requestHeaders: { "User-Agent": "Kibo-Agent/$appVersion" },
  },
  // The ones most people configure first, so they are not hunted for in a list
  // of two hundred. Nothing special about them beyond the place: the same name,
  // protocol and address models.dev has, which `KNOWN_PROVIDERS` then leaves out
  // of its own copy so that each appears once.
  { id: "openai", name: "OpenAI", kind: "openAiCompatible", baseUrl: "https://api.openai.com/v1" },
  { id: "anthropic", name: "Anthropic", kind: "anthropic", baseUrl: "https://api.anthropic.com/v1" },
  { id: "deepseek", name: "DeepSeek", kind: "openAiCompatible", baseUrl: "https://api.deepseek.com" },
  // The one that runs on the machine, and so the one models.dev has no row for
  // at all — its plain `http` is what Ollama serves, not a slip.
  { id: "ollama", name: "Ollama", kind: "openAiCompatible", baseUrl: "http://localhost:11434/v1" },
];

/**
 * Everything the form offers by name: the app's own rows first, then models.dev's
 * — less the ones the app already carries, so that a name is offered once.
 *
 * ponytail: the certificate travels in `settings.json` inside the provider it
 * was picked for, so a new root from the bank needs the row picked again, or
 * the new PEM pasted into the Certificate card. The way past that ceiling is
 * an id in the config that the backend resolves against a file in the build
 * (docs/47-alfagen-provider.md, decision 4) — worth it only once a second
 * internal provider or a changed CA makes it so.
 */
export const KNOWN_PROVIDERS: BuiltinProvider[] = [
  ...BUILTIN_PROVIDERS,
  ...CATALOG.filter((entry) => !isBuiltIn(entry.id)),
];

/** Whether the app carries this row itself rather than reading it from models.dev. */
export function isBuiltIn(id: string): boolean {
  return BUILTIN_PROVIDERS.some((provider) => provider.id === id);
}

/**
 * The built-in provider whose certificate this is, when it is one of theirs:
 * the form then says where the certificate came from rather than showing an
 * unexplained block of PEM.
 */
export function builtinSourceOf(cert: string): BuiltinProvider | undefined {
  const pem = cert.trim();
  return pem ? BUILTIN_PROVIDERS.find((provider) => provider.trustedCertPem === pem) : undefined;
}
