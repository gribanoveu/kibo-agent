import { afterEach, describe, expect, mock, test } from "bun:test";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { Onboarding } from "../components/Onboarding";
import { useOnboarding } from "../hooks/useOnboarding";
import { shouldOnboard, isDeepSeek } from "../lib/onboarding";
import { IS_MAC } from "../lib/shortcuts";
import { useShortcuts } from "../hooks/useShortcuts";
import { DEFAULT_TURN_LIMITS, type LlmSettings, type ProviderConfig, type ProviderView } from "../lib/settings";
import type { FeatureView } from "../lib/features";

const provider = (id: string): ProviderView => ({ id, baseUrl: "https://example.test/v1", hasApiKey: false });

const settings = (providers: ProviderView[] = []): LlmSettings => ({
  providers,
  activeProviderId: providers[0]?.id ?? null,
  debugLogging: false,
  replyLanguage: "auto",
  turnLimits: DEFAULT_TURN_LIMITS,
  keyStore: "file",
});

const feature = (id: string, name: string, on: boolean, defaultOn: boolean): FeatureView => ({
  id,
  name,
  description: `${name} in a sentence or two`,
  summary: `${name} in one line`,
  on,
  defaultOn,
  tools: [],
  toolsOn: true,
  layoutSwitchOn: false,
  layoutSwitchSupported: false,
  places: [],
  data: null,
  hasData: false,
});

const extras = (over: Partial<Parameters<typeof Onboarding>[0]["features"]> = {}) => ({
  list: [
    feature("provider-balance", "Provider balance", false, false),
    feature("deepseek-search", "DeepSeek search", false, false),
    feature("meeting-notes", "Meeting notes", false, false),
    feature("voice-memory", "Voice memory", false, false),
    feature("mongodb", "MongoDB", false, false),
    feature("voice-input", "Voice input", false, false),
    // On by default: the app's normal surface, not an extra to ask about.
    feature("board", "Board", true, true),
  ],
  onSet: mock((_id: string, _on: boolean) => {}),
  error: null,
  ...over,
});

const saving = () => mock(async (_p: ProviderConfig, _k: string | null, _from: string | null) => true);

const ops = (over: Partial<Parameters<typeof Onboarding>[0]["provider"]> = {}) => ({
  busy: false,
  error: null,
  onSave: saving(),
  onProbe: mock(async (_p: ProviderConfig, _k: string | null) => ["a-model", "b-model"]),
  ...over,
});

const screenProps = (over: Parameters<typeof ops>[0] = {}, extrasOver: Parameters<typeof extras>[0] = {}) => ({
  provider: ops(over),
  features: extras(extrasOver),
  onSkip: mock(() => {}),
  onFinish: mock(() => {}),
  onTour: mock(() => {}),
});

const open = (over: Parameters<typeof ops>[0] = {}, extrasOver: Parameters<typeof extras>[0] = {}) => {
  const props = screenProps(over, extrasOver);
  render(<Onboarding {...props} />);
  return props;
};

/** The provider step, reached the way a user reaches it. */
const atProviderStep = (over: Parameters<typeof ops>[0] = {}, extrasOver: Parameters<typeof extras>[0] = {}) => {
  const props = open(over, extrasOver);
  fireEvent.click(screen.getByRole("button", { name: "Get started" }));
  return props;
};

const card = (name: RegExp) => screen.getByRole("button", { name });

// A field's own label, and its hint when it has one — a wrapping label's text
// is both.
const field = (label: string | RegExp) => screen.getByLabelText(label) as HTMLInputElement;

const button = (name: string | RegExp) => screen.getByRole("button", { name }) as HTMLButtonElement;

/** The last step, reached the way a user reaches it: a provider connected. */
const atExtrasStep = async (over: Parameters<typeof ops>[0] = {}, extrasOver: Parameters<typeof extras>[0] = {}) => {
  const props = atProviderStep(over, extrasOver);
  fireEvent.click(card(/OpenAI/));
  fireEvent.click(button("Continue"));
  await waitFor(() => expect(button("Connect")).toBeTruthy());
  fireEvent.click(button("Connect"));
  await waitFor(() => expect(screen.getByText("A few extras")).toBeTruthy());
  return props;
};

afterEach(() => localStorage.clear());

describe("when the screen is due", () => {
  test("not while the settings are still being read", () => {
    expect(shouldOnboard(null, false)).toBe(false);
  });

  test("when nothing can answer yet", () => {
    expect(shouldOnboard(settings(), false)).toBe(true);
  });

  test("not with a provider saved", () => {
    expect(shouldOnboard(settings([provider("openai")]), false)).toBe(false);
  });

  test("not after it was skipped", () => {
    expect(shouldOnboard(settings(), true)).toBe(false);
  });
});

describe("the host that means DeepSeek", () => {
  test("its own host, on any path — and nothing in front of it", () => {
    expect(isDeepSeek("https://api.deepseek.com")).toBe(true);
    expect(isDeepSeek("https://api.deepseek.com/v1/")).toBe(true);
    expect(isDeepSeek(" https://API.deepseek.com ")).toBe(true);
    expect(isDeepSeek("https://api.deepseek.com.evil.example/v1")).toBe(false);
    expect(isDeepSeek("https://gateway.example/api.deepseek.com")).toBe(false);
    expect(isDeepSeek("https://openrouter.ai/api/v1")).toBe(false);
    expect(isDeepSeek("not a url")).toBe(false);
    expect(isDeepSeek("")).toBe(false);
  });
});

describe("the keys of the window behind it", () => {
  test("are held while the screen is up, and let go with it", () => {
    const spy = mock(() => {});
    const press = () =>
      fireEvent.keyDown(window, { code: "Slash", key: "/", ctrlKey: !IS_MAC, metaKey: IS_MAC });
    // A pane behind the layer, still mounted and still listening on the window.
    function Keys() {
      useShortcuts({ shortcuts: spy });
      return null;
    }
    const props = screenProps();

    const tree = render(<Keys />);
    press();
    expect(spy).toHaveBeenCalledTimes(1);

    // Up, the screen keeps the press to itself: nothing opens behind it.
    tree.rerender(
      <>
        <Keys />
        <Onboarding {...props} />
      </>,
    );
    press();
    expect(spy).toHaveBeenCalledTimes(1);

    tree.rerender(<Keys />);
    press();
    expect(spy).toHaveBeenCalledTimes(2);
  });
});

describe("the offer of a tour", () => {
  test("Show me around hands the window over, and closes the screen", async () => {
    const props = await atExtrasStep();
    fireEvent.click(button("Next"));
    fireEvent.click(button("Show me around"));
    expect(props.onTour).toHaveBeenCalledTimes(1);
    expect(props.onFinish).toHaveBeenCalledTimes(1);
  });

  test("Start working closes the screen without a tour", async () => {
    const props = await atExtrasStep();
    fireEvent.click(button("Next"));
    fireEvent.click(button("Start working"));
    expect(props.onFinish).toHaveBeenCalledTimes(1);
    expect(props.onTour).not.toHaveBeenCalled();
  });
});

function Harness({ llm }: { llm: LlmSettings | null }) {
  const { open, dismiss } = useOnboarding(llm);
  return (
    <div>
      <span>{open ? "open" : "closed"}</span>
      <button type="button" onClick={dismiss}>
        skip
      </button>
    </div>
  );
}

const state = () => screen.getByText(/^(open|closed)$/).textContent;

describe("useOnboarding", () => {
  test("opens once the settings arrive without a provider", async () => {
    const { rerender } = render(<Harness llm={null} />);
    expect(state()).toBe("closed");
    rerender(<Harness llm={settings()} />);
    await waitFor(() => expect(state()).toBe("open"));
  });

  test("stays closed for a launch that found a provider", async () => {
    render(<Harness llm={settings([provider("openai")])} />);
    await waitFor(() => expect(state()).toBe("closed"));
  });

  test("a skip is remembered", async () => {
    render(<Harness llm={settings()} />);
    await waitFor(() => expect(state()).toBe("open"));
    fireEvent.click(button("skip"));
    expect(state()).toBe("closed");
    expect(localStorage.getItem("atlas-onboarding-dismissed")).toBe("true");
  });

  test("a window that already skipped never opens", async () => {
    localStorage.setItem("atlas-onboarding-dismissed", "true");
    render(<Harness llm={settings()} />);
    await waitFor(() => expect(state()).toBe("closed"));
  });

  test("removing the last provider mid-launch does not throw it up", async () => {
    const { rerender } = render(<Harness llm={settings([provider("openai")])} />);
    await waitFor(() => expect(state()).toBe("closed"));
    // Read once: what later settings say is not a reason to ask again.
    rerender(<Harness llm={settings()} />);
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(state()).toBe("closed");
  });
});

describe("the welcome step", () => {
  test("comes first, and Get started moves on", () => {
    open();
    expect(screen.getByText("Welcome to Kibo Agent")).toBeTruthy();
    fireEvent.click(button("Get started"));
    expect(screen.getByText("Connect a model")).toBeTruthy();
  });

  test("Skip for now on the welcome step", () => {
    const props = open();
    fireEvent.click(button("Skip for now"));
    expect(props.onSkip).toHaveBeenCalledTimes(1);
  });

  test("the corner Skip leaves from any step", () => {
    const props = atProviderStep();
    fireEvent.click(button("Skip"));
    expect(props.onSkip).toHaveBeenCalledTimes(1);
  });
});

describe("choosing a provider and connecting it", () => {
  test("fills the form from the card picked", () => {
    atProviderStep();
    fireEvent.click(card(/OpenAI/));
    expect(field("Name").value).toBe("openai");
    expect(field("Base URL").value).toBe("https://api.openai.com/v1");
  });

  test("the card's protocol and address come with it", async () => {
    const props = atProviderStep();
    fireEvent.click(card(/Anthropic/));
    fireEvent.click(button("Continue"));
    await waitFor(() => expect(props.provider.onProbe).toHaveBeenCalled());
    expect((props.provider.onProbe as ReturnType<typeof mock>).mock.calls[0][0]).toMatchObject({
      id: "anthropic",
      kind: "anthropic",
      baseUrl: "https://api.anthropic.com/v1",
    });
  });

  test("an endpoint of its own needs both a name and an address", () => {
    atProviderStep();
    fireEvent.click(card(/Other/));
    expect(field("Name").value).toBe("");
    expect(button("Continue").disabled).toBe(true);
    fireEvent.change(field("Name"), { target: { value: "mine" } });
    expect(button("Continue").disabled).toBe(true);
    fireEvent.change(field("Base URL"), { target: { value: "https://llm.corp.test/v1" } });
    expect(button("Continue").disabled).toBe(false);
  });

  test("an endpoint of its own picks which wire it speaks", async () => {
    const props = atProviderStep();
    fireEvent.click(card(/Other/));
    fireEvent.click(screen.getByRole("radio", { name: "Anthropic" }));
    fireEvent.change(field("Name"), { target: { value: "corp" } });
    fireEvent.change(field("Base URL"), { target: { value: "https://llm.corp.test" } });
    fireEvent.click(button("Continue"));

    await waitFor(() => expect(props.provider.onProbe).toHaveBeenCalledTimes(1));
    expect((props.provider.onProbe as ReturnType<typeof mock>).mock.calls[0][0]).toMatchObject({
      id: "corp",
      kind: "anthropic",
      baseUrl: "https://llm.corp.test",
    });
  });

  test("a card brings its own protocol, with nothing to choose", () => {
    atProviderStep();
    fireEvent.click(card(/OpenAI/));
    expect(screen.queryByRole("radiogroup", { name: "Protocol" })).toBeNull();
  });

  test("a chosen provider is named by its mark, not by a heading", () => {
    atProviderStep();
    // Nothing chosen yet, so nothing to mark.
    expect(document.querySelector("svg.provider-logo")).toBeNull();

    fireEvent.click(card(/OpenAI/));
    expect(document.querySelector("svg.provider-logo")).toBeTruthy();
    // The mark and the address under it say which provider this is.
    expect(screen.queryByText(/Connect OpenAI/)).toBeNull();
    expect(screen.queryByText("Your own endpoint")).toBeNull();
  });

  test("another provider is another check: the old model list is not carried over", async () => {
    atProviderStep();
    fireEvent.click(card(/OpenAI/));
    fireEvent.click(button("Continue"));
    await waitFor(() => expect(screen.getByRole("radiogroup", { name: "Model" })).toBeTruthy());

    fireEvent.click(button("Change"));
    fireEvent.click(card(/DeepSeek/));
    expect(screen.queryByRole("radiogroup", { name: "Model" })).toBeNull();
    expect(button("Continue")).toBeTruthy();
  });

  test("a card comes with its address, so there is nothing to fill", () => {
    atProviderStep();
    fireEvent.click(card(/Anthropic/));
    expect(field("Name").value).toBe("anthropic");
    expect(button("Continue").disabled).toBe(false);
  });

  test("checks the key, then saves the model picked", async () => {
    const props = atProviderStep();
    fireEvent.click(card(/DeepSeek/));
    fireEvent.change(field(/API key/), { target: { value: "  sk-1  " } });
    fireEvent.click(button("Continue"));

    await waitFor(() => expect(screen.getByRole("radiogroup", { name: "Model" })).toBeTruthy());
    expect(props.provider.onProbe).toHaveBeenCalledTimes(1);
    expect((props.provider.onProbe as ReturnType<typeof mock>).mock.calls[0][1]).toBe("sk-1");
    // The first model it lists is what the provider's default would send.
    expect(screen.getByRole("radio", { name: "a-model" }).getAttribute("aria-checked")).toBe("true");

    fireEvent.click(button("Connect"));
    // Saved, and on to the extras: closing the screen is the last step's job.
    await waitFor(() => expect(screen.getByText("A few extras")).toBeTruthy());
    expect(props.onFinish).not.toHaveBeenCalled();
    fireEvent.click(button("Next"));
    fireEvent.click(button("Start working"));
    expect(props.onFinish).toHaveBeenCalledTimes(1);
    const [saved, key] = (props.provider.onSave as ReturnType<typeof mock>).mock.calls[0];
    expect(saved).toMatchObject({ id: "deepseek", baseUrl: "https://api.deepseek.com", model: "a-model" });
    expect(key).toBe("sk-1");
  });

  test("pins the model picked rather than the first", async () => {
    const props = atProviderStep();
    fireEvent.click(card(/OpenAI/));
    fireEvent.click(button("Continue"));
    await waitFor(() => expect(screen.getByRole("radio", { name: "b-model" })).toBeTruthy());
    fireEvent.click(screen.getByRole("radio", { name: "b-model" }));
    fireEvent.click(button("Connect"));
    await waitFor(() => expect(screen.getByText("A few extras")).toBeTruthy());
    expect((props.provider.onSave as ReturnType<typeof mock>).mock.calls[0][0].model).toBe("b-model");
  });

  test("a refused check is shown, saves nothing, and can be saved anyway", async () => {
    const props = atProviderStep({
      onProbe: mock(async () => {
        throw new Error("401 Unauthorized");
      }),
    });
    fireEvent.click(card(/OpenAI/));
    fireEvent.click(button("Continue"));

    await waitFor(() => expect(screen.getByRole("alert").textContent).toContain("401 Unauthorized"));
    expect(props.provider.onSave).not.toHaveBeenCalled();

    fireEvent.click(button("Save anyway"));
    await waitFor(() => expect(screen.getByText("A few extras")).toBeTruthy());
    // Saved without a pin: "auto" is the provider's own default.
    expect((props.provider.onSave as ReturnType<typeof mock>).mock.calls[0][0].model).toBeNull();
  });

  test("carries the card's certificate and headers into what is saved", async () => {
    const props = atProviderStep({ onProbe: mock(async () => []) });
    fireEvent.click(card(/AlfaGen/));
    fireEvent.click(button("Continue"));
    await waitFor(() => expect(button("Connect")).toBeTruthy());
    fireEvent.click(button("Connect"));

    await waitFor(() => expect(screen.getByText("A few extras")).toBeTruthy());
    const saved = (props.provider.onSave as ReturnType<typeof mock>).mock.calls[0][0] as ProviderConfig;
    expect(saved.trustedCertPem).toContain("BEGIN CERTIFICATE");
    expect(saved.requestHeaders?.["User-Agent"]).toContain("Kibo-Agent");
  });

  test("asks once, and is busy until the provider answers", async () => {
    let release: (list: string[]) => void = () => {};
    const props = atProviderStep({ onProbe: mock(() => new Promise<string[]>((resolve) => (release = resolve))) });
    fireEvent.click(card(/OpenAI/));
    fireEvent.click(button("Continue"));
    // Nothing to press twice: the answer is what decides what comes next.
    expect(button("Checking…").disabled).toBe(true);
    expect((props.provider.onProbe as ReturnType<typeof mock>).mock.calls.length).toBe(1);

    release(["solo"]);
    await waitFor(() => expect(button("Connect")).toBeTruthy());
    expect(screen.getByRole("radio", { name: "solo" }).getAttribute("aria-checked")).toBe("true");
  });

  test("changing the key after a passing check asks for a check again", async () => {
    atProviderStep();
    fireEvent.click(card(/OpenAI/));
    fireEvent.click(button("Continue"));
    await waitFor(() => expect(screen.getByRole("radiogroup", { name: "Model" })).toBeTruthy());

    fireEvent.change(field(/API key/), { target: { value: "sk-2" } });
    expect(screen.queryByRole("radiogroup", { name: "Model" })).toBeNull();
  });

  test("changing the address after a passing check asks for a check again", async () => {
    atProviderStep();
    fireEvent.click(card(/OpenAI/));
    fireEvent.click(button("Continue"));
    await waitFor(() => expect(screen.getByRole("radiogroup", { name: "Model" })).toBeTruthy());

    fireEvent.change(field("Base URL"), { target: { value: "https://api.openai.com/v2" } });
    expect(screen.queryByRole("radiogroup", { name: "Model" })).toBeNull();
    expect(button("Continue")).toBeTruthy();
  });

  test("a key typed for one provider does not follow to the next", async () => {
    const props = atProviderStep();
    fireEvent.click(card(/OpenAI/));
    fireEvent.change(field(/API key/), { target: { value: "sk-secret" } });

    fireEvent.click(button("Change"));
    fireEvent.click(card(/DeepSeek/));
    expect(field(/API key/).value).toBe("");
    fireEvent.click(button("Continue"));
    await waitFor(() => expect(props.provider.onProbe).toHaveBeenCalledTimes(1));
    expect((props.provider.onProbe as ReturnType<typeof mock>).mock.calls[0][1]).toBe("");
  });

  test("Enter in a field does what the button does", async () => {
    const props = atProviderStep();
    fireEvent.click(card(/OpenAI/));
    fireEvent.keyDown(field(/API key/), { code: "Enter", key: "Enter" });
    await waitFor(() => expect(props.provider.onProbe).toHaveBeenCalledTimes(1));

    await waitFor(() => expect(button("Connect")).toBeTruthy());
    fireEvent.keyDown(field("Name"), { code: "Enter", key: "Enter" });
    await waitFor(() => expect(screen.getByText("A few extras")).toBeTruthy());
    fireEvent.click(button("Next"));
    fireEvent.click(button("Start working"));
    expect(props.onFinish).toHaveBeenCalledTimes(1);
  });

  test("never saves under a refusal from the backend", async () => {
    const props = atProviderStep({
      onProbe: mock(async () => []),
      onSave: mock(async () => false),
    });
    fireEvent.click(card(/OpenAI/));
    fireEvent.click(button("Continue"));
    await waitFor(() => expect(button("Connect")).toBeTruthy());
    fireEvent.click(button("Connect"));
    await waitFor(() => expect(props.provider.onSave).toHaveBeenCalledTimes(1));
    expect(props.onFinish).not.toHaveBeenCalled();
    // Not on to the last step either: nothing was saved to be ready with.
    expect(screen.queryByText("A few extras")).toBeNull();
  });

  test("the catalog searches the providers the cards do not show", () => {
    atProviderStep();
    fireEvent.click(button(/more providers/));
    const search = () => screen.getByPlaceholderText(/Search \d+ providers/) as HTMLInputElement;
    fireEvent.change(search(), { target: { value: "groq" } });
    expect(screen.getAllByRole("button", { name: /groq/i }).length).toBeGreaterThan(0);

    // Enter takes the first match, as the settings form's own list does.
    fireEvent.keyDown(search(), { code: "Enter", key: "Enter" });
    expect(field("Name").value).toBe("groq");

    // And a press on the row is the same pick. The list stays open behind the
    // cards, so the way back finds it there.
    fireEvent.click(button("Change"));
    fireEvent.change(search(), { target: { value: "groq" } });
    fireEvent.click(screen.getAllByRole("button", { name: /groq/i })[0]);
    expect(field("Name").value).toBe("groq");
  });

  test("a provider picked takes the cards' place, and Change brings them back", () => {
    atProviderStep();
    fireEvent.click(card(/OpenAI/));

    // The connection has the window now, and the cards are out of the way.
    expect(screen.queryByRole("button", { name: /Anthropic/ })).toBeNull();
    expect(field("Name").value).toBe("openai");

    fireEvent.click(button("Change"));
    expect(screen.getByRole("button", { name: /Anthropic/ })).toBeTruthy();
    expect(screen.queryByLabelText("Name")).toBeNull();

    fireEvent.click(card(/Anthropic/));
    expect(field("Name").value).toBe("anthropic");
  });
});

describe("the extras", () => {
  test("offers the parts the app leaves off, and only those", async () => {
    await atExtrasStep();
    expect(screen.getByText("Meeting notes in one line")).toBeTruthy();
    expect(screen.getByText("MongoDB in one line")).toBeTruthy();
    expect(screen.getByText("Voice input in one line")).toBeTruthy();
    // Board is on by default: the app's normal surface, not an extra to ask about.
    expect(screen.queryByText("Board in one line")).toBeNull();
    // Voice prints of other people are consented to in the Catalog, not here.
    expect(screen.queryByText("Voice memory in one line")).toBeNull();
    // The connection page is kept for the way back, not drawn over this one.
    expect(screen.queryByRole("button", { name: "Connect" })).toBeNull();
  });

  test("a provider that is not DeepSeek is not offered DeepSeek's two", async () => {
    await atExtrasStep();
    expect(screen.queryByText("Provider balance in one line")).toBeNull();
    expect(screen.queryByText("DeepSeek search in one line")).toBeNull();
    expect(screen.getByText("Voice input in one line")).toBeTruthy();
  });

  test("a DeepSeek provider is offered the two that are its own", async () => {
    atProviderStep();
    fireEvent.click(card(/DeepSeek/));
    fireEvent.click(button("Continue"));
    await waitFor(() => expect(button("Connect")).toBeTruthy());
    fireEvent.click(button("Connect"));
    await waitFor(() => expect(screen.getByText("A few extras")).toBeTruthy());

    expect(screen.getByText("Provider balance in one line")).toBeTruthy();
    expect(screen.getByText("DeepSeek search in one line")).toBeTruthy();
  });

  test("a card turns one on, and says which", async () => {
    const props = await atExtrasStep();
    const card = screen.getByRole("button", { name: /Voice input/ });
    expect(card.getAttribute("aria-pressed")).toBe("false");

    fireEvent.click(card);
    expect((props.features.onSet as ReturnType<typeof mock>).mock.calls[0]).toEqual(["voice-input", true]);
  });

  test("one already on is turned off", async () => {
    const props = await atExtrasStep({}, { list: [feature("mongodb", "MongoDB", true, false)] });
    const card = screen.getByRole("button", { name: /MongoDB/ });
    expect(card.getAttribute("aria-pressed")).toBe("true");

    fireEvent.click(card);
    expect((props.features.onSet as ReturnType<typeof mock>).mock.calls[0]).toEqual(["mongodb", false]);
  });

  test("a choice that could not be saved says why", async () => {
    await atExtrasStep({}, { error: "the settings file is read-only" });
    expect(screen.getByText("the settings file is read-only")).toBeTruthy();
  });

  test("Next goes on to the offer of the tour", async () => {
    await atExtrasStep();
    fireEvent.click(button("Next"));
    expect(screen.getByText("One more thing")).toBeTruthy();
  });

  test("Back from the offer returns to the extras", async () => {
    await atExtrasStep();
    fireEvent.click(button("Next"));
    fireEvent.click(button("Back"));
    expect(screen.getByText("Voice input in one line")).toBeTruthy();
  });

  test("Back from the last step keeps the form as it was saved", async () => {
    const props = await atProviderStep();
    fireEvent.click(card(/OpenAI/));
    fireEvent.change(field(/API key/), { target: { value: "sk-kept" } });
    fireEvent.click(button("Continue"));
    await waitFor(() => expect(button("Connect")).toBeTruthy());
    fireEvent.click(button("Connect"));
    await waitFor(() => expect(screen.getByText("A few extras")).toBeTruthy());

    fireEvent.click(button("Back"));
    // Not a blank form: an empty key deletes the one that was just stored.
    expect(field("Name").value).toBe("openai");
    expect(field(/API key/).value).toBe("sk-kept");

    fireEvent.click(button("Connect"));
    await waitFor(() => expect(screen.getByText("A few extras")).toBeTruthy());
    expect((props.provider.onSave as ReturnType<typeof mock>).mock.calls[1][1]).toBe("sk-kept");
  });
});
