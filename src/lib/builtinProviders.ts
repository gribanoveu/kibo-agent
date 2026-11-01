// The providers the app carries itself, offered beside the catalog generated
// from models.dev: a row here may also name the certificate its gateway needs,
// which the generated list has no field for.

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
 * fields for.
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
    // update does not leave a stale version in the settings.
    requestHeaders: { "User-Agent": "Kibo-Agent/$appVersion" },
  },
];

/**
 * Everything the form offers by name, the app's own rows first.
 *
 * ponytail: the certificate travels in `settings.json` inside the provider it
 * was picked for, so a new root from the bank needs the row picked again, or
 * the new PEM pasted into the Certificate card. The way past that ceiling is
 * an id in the config that the backend resolves against a file in the build
 * (docs/47-alfagen-provider.md, decision 4) — worth it only once a second
 * internal provider or a changed CA makes it so.
 */
export const KNOWN_PROVIDERS: BuiltinProvider[] = [...BUILTIN_PROVIDERS, ...CATALOG];

/**
 * The built-in provider whose certificate this is, when it is one of theirs:
 * the form then says where the certificate came from rather than showing an
 * unexplained block of PEM.
 */
export function builtinSourceOf(cert: string): BuiltinProvider | undefined {
  const pem = cert.trim();
  return pem ? BUILTIN_PROVIDERS.find((provider) => provider.trustedCertPem === pem) : undefined;
}
