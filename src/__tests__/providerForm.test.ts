import { describe, expect, test } from "bun:test";
import { CATALOG, certificateCount, filterModels, freeName, pemFromBytes } from "../lib/providerForm";
import { BUILTIN_PROVIDERS, KNOWN_PROVIDERS, builtinSourceOf, isBuiltIn } from "../lib/builtinProviders";

describe("filterModels", () => {
  const models = ["gpt-4o", "claude-sonnet-4-5", "claude-opus-4-1", "Qwen3-Coder"];

  test("every word has to match, in any order and case", () => {
    expect(filterModels(models, "4 SONNET")).toEqual(["claude-sonnet-4-5"]);
    expect(filterModels(models, "claude")).toEqual(["claude-sonnet-4-5", "claude-opus-4-1"]);
    expect(filterModels(models, "qwen coder")).toEqual(["Qwen3-Coder"]);
  });

  test("an empty search leaves everything", () => {
    expect(filterModels(models, "  ")).toEqual(models);
  });
});

describe("the provider catalog", () => {
  // The snapshot is regenerated from models.dev; what the form relies on is checked here.
  test("is unique names, HTTPS URLs and a protocol the app speaks", () => {
    expect(new Set(CATALOG.map((p) => p.id)).size).toBe(CATALOG.length);
    for (const p of CATALOG) {
      expect(p.baseUrl).toStartWith("https://");
      expect(["openAiCompatible", "anthropic"]).toContain(p.kind);
    }
    expect(CATALOG.find((p) => p.id === "anthropic")?.kind).toBe("anthropic");
    expect(CATALOG.find((p) => p.id === "openai")?.baseUrl).toBe("https://api.openai.com/v1");
  });

  test("a taken name gets the next free number", () => {
    expect(freeName("deepseek", [])).toBe("deepseek");
    expect(freeName("deepseek", ["deepseek", "deepseek-2"])).toBe("deepseek-3");
  });
});

describe("the providers the app carries", () => {
  const alfagen = BUILTIN_PROVIDERS.find((provider) => provider.id === "alfagen");

  test("AlfaGen is offered at the bank's own address, on the protocol it speaks", () => {
    expect(alfagen?.baseUrl).toBe("https://alfagen.moscow.alfaintra.net/continue-dev/v1");
    expect(alfagen?.kind).toBe("openAiCompatible");
  });

  /// One certificate, the root the gateway's chain ends at: the server sends
  /// its intermediate itself, and a bundled leaf would be pinned to the
  /// certificate that expires first.
  test("and with exactly one certificate — the root", () => {
    expect(certificateCount(alfagen?.trustedCertPem ?? "")).toBe(1);
    expect(alfagen?.trustedCertPem).toStartWith("-----BEGIN CERTIFICATE-----");
    expect(alfagen?.trustedCertPem).toEndWith("-----END CERTIFICATE-----");
  });

  test("a certificate is the app's own only when it is that one", () => {
    expect(builtinSourceOf(alfagen?.trustedCertPem ?? "")?.id).toBe("alfagen");
    expect(builtinSourceOf("  ")).toBeUndefined();
    expect(builtinSourceOf("-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----")).toBeUndefined();
  });

  /// The gateway tells its clients apart by `User-Agent`, and the version in
  /// it has to be the one of the build that sends it — hence a placeholder,
  /// expanded by the backend on each request, not a version copied in here.
  test("and with the User-Agent its gateway asks for, as a placeholder", () => {
    expect(alfagen?.requestHeaders).toEqual({ "User-Agent": "Kibo-Agent/$appVersion" });
  });

  test("are offered ahead of the catalog, each name once", () => {
    expect(KNOWN_PROVIDERS.slice(0, BUILTIN_PROVIDERS.length)).toEqual(BUILTIN_PROVIDERS);
    const ids = KNOWN_PROVIDERS.map((provider) => provider.id);
    expect(new Set(ids).size).toBe(ids.length);
    const catalogOnly = CATALOG.filter((entry) => !BUILTIN_PROVIDERS.some((builtin) => builtin.id === entry.id));
    expect(KNOWN_PROVIDERS).toHaveLength(BUILTIN_PROVIDERS.length + catalogOnly.length);
  });

  /// The four nearly everyone configures come before a catalog of two hundred,
  /// and bring nothing but their place: no certificate, no headers.
  test("the ones configured first are at the top", () => {
    expect(BUILTIN_PROVIDERS.map((provider) => provider.id)).toEqual([
      "alfagen",
      "openai",
      "anthropic",
      "deepseek",
      "ollama",
    ]);
    expect(BUILTIN_PROVIDERS.find((provider) => provider.id === "anthropic")?.kind).toBe("anthropic");
    for (const provider of BUILTIN_PROVIDERS.slice(1)) {
      expect(provider.trustedCertPem).toBeUndefined();
      expect(provider.requestHeaders).toBeUndefined();
    }
  });

  /// The one that runs on the machine: models.dev lists only its cloud, and
  /// plain `http` is how a local Ollama answers.
  test("Ollama is the local address, which models.dev does not list", () => {
    const ollama = BUILTIN_PROVIDERS.find((provider) => provider.id === "ollama");
    expect(ollama?.baseUrl).toBe("http://localhost:11434/v1");
    expect(CATALOG.some((entry) => entry.id === "ollama")).toBe(false);
  });

  /// The app's row replaces models.dev's copy of the same name rather than
  /// joining it: two rows called OpenAI, one above the other, is how a list of
  /// known providers turns into a puzzle.
  test("a name the app carries is offered as the app's row", () => {
    expect(isBuiltIn("openai")).toBe(true);
    expect(isBuiltIn("ollama")).toBe(true);
    expect(isBuiltIn("ollama-cloud")).toBe(false);
    const openai = KNOWN_PROVIDERS.filter((provider) => provider.id === "openai");
    expect(openai).toHaveLength(1);
    expect(openai[0]).toBe(BUILTIN_PROVIDERS.find((provider) => provider.id === "openai"));
  });
});

describe("certificates", () => {
  test("are counted per block", () => {
    const block = "-----BEGIN CERTIFICATE-----\nAA\n-----END CERTIFICATE-----\n";
    expect(certificateCount("")).toBe(0);
    expect(certificateCount(`subject=x\n${block}${block}`)).toBe(2);
  });

  test("PEM text is taken as it is", () => {
    const pem = "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----";
    expect(pemFromBytes(new TextEncoder().encode(`\n${pem}\n`))).toBe(pem);
  });

  /// A `.cer` is often binary DER, which the backend reads only as PEM.
  test("binary DER is wrapped as PEM, 64 columns a line", () => {
    const der = new Uint8Array(100).map((_, i) => (i * 37) % 256);
    const pem = pemFromBytes(der);
    const lines = pem.split("\n");
    expect(lines[0]).toBe("-----BEGIN CERTIFICATE-----");
    expect(lines.at(-1)).toBe("-----END CERTIFICATE-----");
    expect(lines[1]).toHaveLength(64);
    const decoded = atob(lines.slice(1, -1).join(""));
    expect(Uint8Array.from(decoded, (c) => c.charCodeAt(0))).toEqual(der);
  });
});
