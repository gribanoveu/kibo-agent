import { describe, expect, test } from "bun:test";
import { CATALOG, certificateCount, filterModels, freeName, pemFromBytes } from "../lib/providerForm";

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
