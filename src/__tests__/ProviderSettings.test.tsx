import { describe, expect, test } from "bun:test";
import { render, screen, fireEvent, act, cleanup } from "@testing-library/react";
import { ProviderSettings } from "../components/ProviderSettings";
import type { LlmSettings, ProviderConfig } from "../lib/chat";

// The form's one real rule: an untouched key field must leave the stored key
// alone. Everything else here is about not editing a provider that is no
// longer there.

const settings = (over: Partial<LlmSettings> = {}): LlmSettings => ({
  providers: [
    { id: "local", baseUrl: "http://127.0.0.1:1234/v1", model: "qwen", hasApiKey: true },
    { id: "openai", baseUrl: "https://api.openai.com/v1", hasApiKey: false },
  ],
  activeProviderId: "local",
  debugLogging: false,
  ...over,
});

type Saved = { provider: ProviderConfig; apiKey: string | null };

const form = (over: Partial<LlmSettings> = {}) => {
  const saved: Saved[] = [];
  const removed: string[] = [];
  render(
    <ProviderSettings
      settings={settings(over)}
      busy={false}
      error={null}
      onSave={(provider, apiKey) => saved.push({ provider, apiKey })}
      onRemove={(id) => removed.push(id)}
      onSelect={() => {}}
    />,
  );
  return { saved, removed };
};

/** The fields are labelled but not `for`-linked, the way the prototype draws them. */
const field = (label: string) => {
  const fields = Array.from(document.querySelectorAll(".modal-field"));
  const found = fields.find((el) => el.querySelector("label")?.textContent === label);
  return found?.querySelector("input") as HTMLInputElement;
};

describe("editing a provider", () => {
  test("opens on the active one, with its values", () => {
    form();

    expect(field("Name").value).toBe("local");
    expect(field("Base URL").value).toBe("http://127.0.0.1:1234/v1");
    expect(field("Model").value).toBe("qwen");
  });

  /// The whole reason the key field starts empty rather than pre-filled: there
  /// is nothing to pre-fill it with, and sending its empty value on every save
  /// would delete the key the user just stored.
  test("an untouched key field leaves the stored key alone", async () => {
    const { saved } = form();

    fireEvent.change(field("Model"), { target: { value: "qwen3" } });
    await act(async () => {
      fireEvent.click(screen.getByText("Save"));
    });

    expect(saved[0]?.apiKey).toBeNull();
    expect(saved[0]?.provider.model).toBe("qwen3");
  });

  test("a typed key is sent to be sealed", async () => {
    const { saved } = form();

    fireEvent.change(field("API key"), { target: { value: "sk-new" } });
    await act(async () => {
      fireEvent.click(screen.getByText("Save"));
    });

    expect(saved[0]?.apiKey).toBe("sk-new");
  });

  test("the key field says whether one is already stored", () => {
    form();
    expect(field("API key").placeholder).toContain("stored");
  });

  test("an empty model means auto, not an empty string", async () => {
    const { saved } = form();

    fireEvent.change(field("Model"), { target: { value: "  " } });
    await act(async () => {
      fireEvent.click(screen.getByText("Save"));
    });

    expect(saved[0]?.provider.model).toBeNull();
  });

  /// Found by a mutation: without clearing the field on the way out, a key
  /// typed for one provider is sealed under the next one's id — the wrong
  /// endpoint is handed a credential, and the right one still has none.
  test("a key typed for one provider does not follow you to another", async () => {
    const { saved } = form();

    fireEvent.change(field("API key"), { target: { value: "sk-for-local" } });
    fireEvent.click(screen.getByText("openai"));
    await act(async () => {
      fireEvent.click(screen.getByText("Save"));
    });

    expect(saved[0]?.provider.id).toBe("openai");
    expect(saved[0]?.apiKey).toBeNull();
  });

  test("adding one starts from a blank form", async () => {
    const { saved } = form();

    fireEvent.click(screen.getByTitle("Add another provider"));
    expect(field("Name").value).toBe("");

    fireEvent.change(field("Name"), { target: { value: "openrouter" } });
    fireEvent.change(field("Base URL"), { target: { value: "https://openrouter.ai/api/v1" } });
    await act(async () => {
      fireEvent.click(screen.getByText("Save"));
    });

    expect(saved[0]?.provider.id).toBe("openrouter");
  });

  test("a provider is OpenAI-compatible until switched to Anthropic", async () => {
    const { saved } = form();

    expect(screen.getByText("OpenAI-compatible").getAttribute("aria-checked")).toBe("true");
    fireEvent.click(screen.getByText("Anthropic"));
    expect(field("Base URL").placeholder).toBe("https://api.anthropic.com/v1");
    await act(async () => {
      fireEvent.click(screen.getByText("Save"));
    });

    expect(saved[0]?.provider.kind).toBe("anthropic");
  });

  test("reasoning effort is kept, and blank means the model's default", async () => {
    const { saved } = form({
      providers: [{ id: "claude", kind: "anthropic", baseUrl: "u", reasoningEffort: "high", hasApiKey: true }],
      activeProviderId: "claude",
    });
    expect(field("Reasoning effort").value).toBe("high");

    fireEvent.change(field("Reasoning effort"), { target: { value: "  " } });
    await act(async () => {
      fireEvent.click(screen.getByText("Save"));
    });
    expect(saved[0]?.provider.reasoningEffort).toBeNull();
  });

  test("a key typed and left is saved with its provider, but not a cleared one or a nameless form", async () => {
    const { saved } = form();
    fireEvent.change(field("API key"), { target: { value: "sk-typed" } });
    await act(async () => {
      fireEvent.blur(field("API key"));
    });
    expect(saved).toHaveLength(1);
    expect(saved[0]?.provider.id).toBe("local");
    expect(saved[0]?.apiKey).toBe("sk-typed");

    fireEvent.change(field("API key"), { target: { value: "" } });
    fireEvent.blur(field("API key"));
    fireEvent.change(field("Name"), { target: { value: " " } });
    fireEvent.change(field("API key"), { target: { value: "sk-other" } });
    fireEvent.blur(field("API key"));
    expect(saved).toHaveLength(1);
  });

  test("the models URL shows the path it would take, and blank sends none", async () => {
    const { saved } = form({
      providers: [{ id: "deepseek", kind: "anthropic", baseUrl: "https://api.deepseek.com/anthropic/", hasApiKey: true }],
      activeProviderId: "deepseek",
    });
    expect(field("Models URL").placeholder).toBe("https://api.deepseek.com/anthropic/models");

    fireEvent.change(field("Models URL"), { target: { value: " https://api.deepseek.com/models " } });
    await act(async () => {
      fireEvent.click(screen.getByText("Save"));
    });
    fireEvent.change(field("Models URL"), { target: { value: "  " } });
    await act(async () => {
      fireEvent.click(screen.getByText("Save"));
    });
    expect(saved.map((s) => s.provider.modelsUrl)).toEqual(["https://api.deepseek.com/models", null]);
  });

  test("a thinking level is one click, and the box shows what it sends", async () => {
    const { saved } = form({
      providers: [{ id: "gpt", baseUrl: "u", hasApiKey: true }],
      activeProviderId: "gpt",
    });
    const group = screen.getByRole("radiogroup", { name: "Reasoning effort" });
    expect(group.querySelector('[aria-checked="true"]')?.textContent).toBe("Default");

    fireEvent.click(screen.getByRole("radio", { name: "Medium" }));
    expect(field("Reasoning effort").value).toBe("medium");
    await act(async () => {
      fireEvent.click(screen.getByText("Save"));
    });
    expect(saved[0]?.provider.reasoningEffort).toBe("medium");
  });

  test("the context window is 260k unless one was set, and takes digits only", () => {
    form();
    expect(field("Context window").value).toBe("260000");

    fireEvent.change(field("Context window"), { target: { value: "128 000" } });
    expect(field("Context window").value).toBe("128000");
  });

  test("a set context window is kept and sent", async () => {
    const { saved } = form({
      providers: [{ id: "local", baseUrl: "http://127.0.0.1:1234/v1", contextLimit: 32_000, hasApiKey: true }],
    });
    expect(field("Context window").value).toBe("32000");
    await act(async () => {
      fireEvent.click(screen.getByText("Save"));
    });
    expect(saved[0]?.provider.contextLimit).toBe(32_000);
  });

  test("a stored Anthropic provider opens as one", () => {
    form({
      providers: [{ id: "claude", kind: "anthropic", baseUrl: "https://api.anthropic.com/v1", hasApiKey: true }],
      activeProviderId: "claude",
    });
    expect(screen.getByText("Anthropic").getAttribute("aria-checked")).toBe("true");
  });

  test("removing names the provider being edited", async () => {
    const { removed } = form();
    fireEvent.click(screen.getByText("Remove"));
    expect(removed).toEqual(["local"]);
  });
});

describe("with nothing configured", () => {
  test("there is no provider picker, only a blank form", () => {
    render(
      <ProviderSettings
        settings={{ providers: [], activeProviderId: null, debugLogging: false }}
        busy={false}
        error={null}
        onSave={() => {}}
        onRemove={() => {}}
        onSelect={() => {}}
      />,
    );

    expect(screen.queryByText("Remove")).toBeNull();
    expect(field("Name").value).toBe("");
  });
});

describe("saving", () => {
  /// Every other box in this app sends on Enter. One that quietly does nothing
  /// looks exactly like one that saved — which is how a typed key goes missing.
  test("Enter in a field saves, the way it does everywhere else", async () => {
    const { saved } = form();

    fireEvent.change(field("API key"), { target: { value: "sk-typed" } });
    await act(async () => {
      fireEvent.keyDown(field("API key"), { key: "Enter" });
    });

    expect(saved).toHaveLength(1);
    expect(saved[0].apiKey).toBe("sk-typed");
  });

  test("and the form says it was stored", async () => {
    render(
      <ProviderSettings
        settings={settings()}
        busy={false}
        error={null}
        onSave={() => Promise.resolve(true)}
        onRemove={() => {}}
        onSelect={() => {}}
      />,
    );

    expect(screen.queryByText("Saved.")).toBeNull();
    await act(async () => {
      fireEvent.click(screen.getByText("Save"));
    });
    expect(screen.queryByText("Saved.")).toBeTruthy();
  });

  /// A refusal must not read as a success.
  test("a refused save says nothing", async () => {
    render(
      <ProviderSettings
        settings={settings()}
        busy={false}
        error={null}
        onSave={() => Promise.resolve(false)}
        onRemove={() => {}}
        onSelect={() => {}}
      />,
    );

    await act(async () => {
      fireEvent.click(screen.getByText("Save"));
    });
    expect(screen.queryByText("Saved.")).toBeNull();
  });

  /// And it must not stand over a form that has changed since: "Saved." next
  /// to an edited key is the same lie in the other direction.
  test("editing again takes the word back", async () => {
    render(
      <ProviderSettings
        settings={settings()}
        busy={false}
        error={null}
        onSave={() => Promise.resolve(true)}
        onRemove={() => {}}
        onSelect={() => {}}
      />,
    );

    await act(async () => {
      fireEvent.click(screen.getByText("Save"));
    });
    expect(screen.queryByText("Saved.")).toBeTruthy();

    fireEvent.change(field("API key"), { target: { value: "sk-newer" } });
    expect(screen.queryByText("Saved.")).toBeNull();
  });
});

describe("the model list", () => {
  const many = Array.from({ length: 12 }, (_, i) => `model-${i}`).concat("claude-sonnet-4-5");

  const listed = (served: Record<string, string[]>, onProbe?: (p: ProviderConfig, k: string | null) => Promise<string[]>) => {
    const saved: Saved[] = [];
    render(
      <ProviderSettings
        settings={settings()}
        busy={false}
        error={null}
        onSave={(provider, apiKey) => saved.push({ provider, apiKey })}
        onRemove={() => {}}
        onSelect={() => {}}
        served={served}
        onProbe={onProbe}
      />,
    );
    return saved;
  };
  const saveNow = () =>
    act(async () => {
      fireEvent.click(screen.getByText("Save"));
    });

  test("a listed model is picked as the default and sent", async () => {
    const saved = listed({ local: ["qwen", "llama"] });
    expect(screen.getByText("qwen").closest("button")?.getAttribute("aria-checked")).toBe("true");

    fireEvent.click(screen.getByText("llama"));
    await saveNow();
    expect(saved[0]?.provider.model).toBe("llama");
  });

  test("auto is a choice, and sends no model", async () => {
    const saved = listed({ local: ["qwen", "llama"] });
    fireEvent.click(screen.getByText("Auto"));
    await saveNow();
    expect(saved[0]?.provider.model).toBeNull();
  });

  test("a short list has no search, a long one does", () => {
    listed({ local: ["qwen", "llama"] });
    expect(document.querySelector('input[type="search"]')).toBeNull();
    cleanup();
    listed({ local: many });
    expect(document.querySelector('input[type="search"]')).toBeTruthy();
  });

  test("the search narrows by every word, and Enter picks the first match without saving", async () => {
    const saved = listed({ local: many });
    const search = document.querySelector('input[type="search"]') as HTMLInputElement;

    fireEvent.change(search, { target: { value: "sonnet 4" } });
    expect(screen.queryByText("model-3")).toBeNull();
    fireEvent.keyDown(search, { key: "Enter" });
    expect(saved).toHaveLength(0);

    await saveNow();
    expect(saved[0]?.provider.model).toBe("claude-sonnet-4-5");
  });

  test("a name the provider does not list can still be used", async () => {
    const saved = listed({ local: many });
    fireEvent.change(document.querySelector('input[type="search"]') as HTMLInputElement, {
      target: { value: "my-finetune" },
    });
    fireEvent.click(screen.getByText("Use “my-finetune”"));
    await saveNow();
    expect(saved[0]?.provider.model).toBe("my-finetune");
  });

  /// The pinned model is what the next turn sends: gone from the list is not gone.
  test("a default the provider no longer lists stays in view", () => {
    listed({ local: ["llama"] });
    expect(screen.getByText("not listed")).toBeTruthy();
    expect(screen.getByText("qwen").closest("button")?.getAttribute("aria-checked")).toBe("true");
  });

  test("refresh asks with the form as typed, key included", async () => {
    const asked: { provider: ProviderConfig; key: string | null }[] = [];
    listed({ local: ["qwen"] }, async (provider, key) => {
      asked.push({ provider, key });
      return ["qwen"];
    });
    fireEvent.change(field("Base URL"), { target: { value: "http://10.0.0.5/v1" } });
    fireEvent.change(field("API key"), { target: { value: "sk-typed" } });
    await act(async () => {
      fireEvent.click(screen.getByText("Refresh"));
    });

    expect(asked.at(-1)?.provider.baseUrl).toBe("http://10.0.0.5/v1");
    expect(asked.at(-1)?.key).toBe("sk-typed");
  });

  test("a stored provider with a key is asked once when opened, and a refusal is shown", async () => {
    const asked: string[] = [];
    await act(async () => {
      listed({}, async (provider) => {
        asked.push(provider.id);
        throw new Error("tls: unknown issuer");
      });
    });
    expect(asked).toEqual(["local"]);
    expect(screen.getByText(/unknown issuer/)).toBeTruthy();
  });
});

describe("sampling", () => {
  const slider = (label: string) => screen.getByLabelText(label) as HTMLInputElement;

  test("unset is sent as nothing, a moved slider as its number", async () => {
    const { saved } = form();
    expect(screen.getAllByText("default")).toHaveLength(2);

    fireEvent.change(slider("Temperature"), { target: { value: "0.3" } });
    await act(async () => {
      fireEvent.click(screen.getByText("Save"));
    });
    expect(saved[0]?.provider.temperature).toBe(0.3);
    expect(saved[0]?.provider.topP).toBeNull();
  });

  test("a stored value opens rounded, and reset returns it to the default", async () => {
    const { saved } = form({
      providers: [{ id: "local", baseUrl: "u", temperature: 0.699999988, topP: 0.9, hasApiKey: true }],
    });
    expect(screen.getByText("0.70")).toBeTruthy();

    fireEvent.click(screen.getByLabelText("Reset Top P"));
    await act(async () => {
      fireEvent.click(screen.getByText("Save"));
    });
    expect(saved[0]?.provider.temperature).toBe(0.7);
    expect(saved[0]?.provider.topP).toBeNull();
  });

  test("Anthropic with both set is warned about", () => {
    form({
      providers: [{ id: "c", kind: "anthropic", baseUrl: "u", temperature: 0.5, topP: 0.9, hasApiKey: true }],
      activeProviderId: "c",
    });
    expect(screen.getByText(/refuse a request that sets both/)).toBeTruthy();
  });
});

describe("the certificate", () => {
  const PEM = "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----";

  test("none is sent as nothing", async () => {
    const { saved } = form();
    expect(screen.getByText(/public roots/)).toBeTruthy();
    await act(async () => {
      fireEvent.click(screen.getByText("Save"));
    });
    expect(saved[0]?.provider.trustedCertPem).toBeNull();
  });

  test("a pasted one is counted and sent", async () => {
    const { saved } = form();
    fireEvent.click(screen.getByText("Paste"));
    fireEvent.change(document.querySelector("textarea") as HTMLTextAreaElement, { target: { value: PEM + PEM } });
    expect(screen.getByText(/2 certificates/)).toBeTruthy();
    await act(async () => {
      fireEvent.click(screen.getByText("Save"));
    });
    expect(saved[0]?.provider.trustedCertPem).toBe(PEM + PEM);
  });

  test("a stored one is kept, and removing it sends nothing", async () => {
    const { saved } = form({ providers: [{ id: "local", baseUrl: "u", trustedCertPem: PEM, hasApiKey: true }] });
    expect(screen.getByText(/1 certificate —/)).toBeTruthy();

    const removes = screen.getAllByText("Remove");
    fireEvent.click(removes[0]); // the certificate's, above the provider's
    await act(async () => {
      fireEvent.click(screen.getByText("Save"));
    });
    expect(saved[0]?.provider.trustedCertPem).toBeNull();
  });
});

describe("where the keys are kept", () => {
  const withStore = (keyStore: LlmSettings["keyStore"]) => {
    const moved: string[] = [];
    render(
      <ProviderSettings
        settings={settings({ keyStore })}
        busy={false}
        error={null}
        onSave={() => {}}
        onRemove={() => {}}
        onSelect={() => {}}
        onKeyStore={(store) => moved.push(store)}
      />,
    );
    return moved;
  };

  test("the file is the default, and the keychain is offered", () => {
    const moved = withStore("file");

    expect(screen.getByText(/sealed in ~\/\.kibo/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Move to keychain" }));
    expect(moved).toEqual(["keychain"]);
  });

  test("keys in the keychain can be moved back", () => {
    const moved = withStore("keychain");

    fireEvent.click(screen.getByRole("button", { name: "Move to ~/.kibo" }));
    expect(moved).toEqual(["file"]);
  });
});
