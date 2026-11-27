import { afterEach, describe, expect, mock, test } from "bun:test";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { Onboarding } from "../components/Onboarding";
import { useOnboarding } from "../hooks/useOnboarding";
import { shouldOnboard } from "../lib/onboarding";
import { DEFAULT_TURN_LIMITS, type LlmSettings, type ProviderConfig, type ProviderView } from "../lib/settings";

const provider = (id: string): ProviderView => ({ id, baseUrl: "https://example.test/v1", hasApiKey: false });

const settings = (providers: ProviderView[] = []): LlmSettings => ({
  providers,
  activeProviderId: providers[0]?.id ?? null,
  debugLogging: false,
  replyLanguage: "auto",
  turnLimits: DEFAULT_TURN_LIMITS,
  keyStore: "file",
});

const folders = (over: Partial<Parameters<typeof Onboarding>[0]["folder"]> = {}) => ({
  recent: [{ path: "/Users/me/work/kibo-agent", worktreeOf: null }],
  onOpen: mock(async (_path: string) => true),
  onPick: mock(async () => true),
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

const screenProps = (over: Parameters<typeof ops>[0] = {}, folderOver: Parameters<typeof folders>[0] = {}) => ({
  provider: ops(over),
  folder: folders(folderOver),
  onSkip: mock(() => {}),
  onFinish: mock(() => {}),
});

const open = (over: Parameters<typeof ops>[0] = {}, folderOver: Parameters<typeof folders>[0] = {}) => {
  const props = screenProps(over, folderOver);
  render(<Onboarding {...props} />);
  return props;
};

/** The provider step, reached the way a user reaches it. */
const atProviderStep = (over: Parameters<typeof ops>[0] = {}, folderOver: Parameters<typeof folders>[0] = {}) => {
  const props = open(over, folderOver);
  fireEvent.click(screen.getByRole("button", { name: "Get started" }));
  return props;
};

const card = (name: RegExp) => screen.getByRole("button", { name });

// A field's own label, and its hint when it has one — a wrapping label's text
// is both.
const field = (label: string | RegExp) => screen.getByLabelText(label) as HTMLInputElement;

const button = (name: string | RegExp) => screen.getByRole("button", { name }) as HTMLButtonElement;

/** The last step, reached the way a user reaches it: a provider connected. */
const atReadyStep = async (over: Parameters<typeof ops>[0] = {}, folderOver: Parameters<typeof folders>[0] = {}) => {
  const props = atProviderStep(over, folderOver);
  fireEvent.click(card(/OpenAI/));
  fireEvent.click(button("Continue"));
  await waitFor(() => expect(button("Connect")).toBeTruthy());
  fireEvent.click(button("Connect"));
  await waitFor(() => expect(screen.getByText("You're set")).toBeTruthy());
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

describe("the provider step", () => {
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

  test("has nothing to send until a provider is picked", () => {
    atProviderStep();
    expect(button("Continue").disabled).toBe(true);
    fireEvent.click(card(/Anthropic/));
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
    // Saved, and on to the last step: closing the screen is Start's job.
    await waitFor(() => expect(screen.getByText("You're set")).toBeTruthy());
    expect(props.onFinish).not.toHaveBeenCalled();
    fireEvent.click(button("Start"));
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
    await waitFor(() => expect(screen.getByText("You're set")).toBeTruthy());
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
    await waitFor(() => expect(screen.getByText("You're set")).toBeTruthy());
    // Saved without a pin: "auto" is the provider's own default.
    expect((props.provider.onSave as ReturnType<typeof mock>).mock.calls[0][0].model).toBeNull();
  });

  test("carries the card's certificate and headers into what is saved", async () => {
    const props = atProviderStep({ onProbe: mock(async () => []) });
    fireEvent.click(card(/AlfaGen/));
    fireEvent.click(button("Continue"));
    await waitFor(() => expect(button("Connect")).toBeTruthy());
    fireEvent.click(button("Connect"));

    await waitFor(() => expect(screen.getByText("You're set")).toBeTruthy());
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
    await waitFor(() => expect(screen.getByText("You're set")).toBeTruthy());
    fireEvent.click(button("Start"));
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
    expect(screen.queryByText("You're set")).toBeNull();
  });

  test("the catalog searches the providers the cards do not show", () => {
    atProviderStep();
    fireEvent.click(card(/Other/));
    const search = () => screen.getByPlaceholderText(/Search \d+ providers/) as HTMLInputElement;
    fireEvent.change(search(), { target: { value: "groq" } });
    expect(screen.getAllByRole("button", { name: /groq/i }).length).toBeGreaterThan(0);

    // Enter takes the first match, as the settings form's own list does.
    fireEvent.keyDown(search(), { code: "Enter", key: "Enter" });
    expect(field("Name").value).toBe("groq");
    // Picked from the list is picked: the search goes away with the choice.
    expect(screen.queryByPlaceholderText(/Search \d+ providers/)).toBeNull();

    // And a press on the row is the same pick.
    fireEvent.click(card(/Other/));
    fireEvent.change(search(), { target: { value: "groq" } });
    fireEvent.click(screen.getAllByRole("button", { name: /groq/i })[0]);
    expect(field("Name").value).toBe("groq");
    expect(screen.queryByPlaceholderText(/Search \d+ providers/)).toBeNull();
  });
});

describe("the last step", () => {
  test("offers the folders opened lately, and closes on one", async () => {
    const props = await atReadyStep();
    expect(screen.getByText("/Users/me/work/kibo-agent")).toBeTruthy();
    // The provider step is kept for the way back, not drawn over this one.
    expect(screen.queryByRole("button", { name: "Connect" })).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: /kibo-agent/ }));
    await waitFor(() => expect(props.onFinish).toHaveBeenCalledTimes(1));
    expect((props.folder.onOpen as ReturnType<typeof mock>).mock.calls[0][0]).toBe("/Users/me/work/kibo-agent");
  });

  test("choosing a folder asks for one, and closes once one is opened", async () => {
    const props = await atReadyStep({}, { onPick: mock(async () => true) });
    fireEvent.click(button("Choose a folder…"));
    await waitFor(() => expect(props.onFinish).toHaveBeenCalledTimes(1));
    expect(props.folder.onPick).toHaveBeenCalledTimes(1);
  });

  test("a cancelled picker leaves the step where it is", async () => {
    const props = await atReadyStep({}, { onPick: mock(async () => false) });
    fireEvent.click(button("Choose a folder…"));
    await waitFor(() => expect(props.folder.onPick).toHaveBeenCalledTimes(1));
    expect(props.onFinish).not.toHaveBeenCalled();
    expect(button("Start")).toBeTruthy();
  });

  test("a folder that would not open keeps the step, and says why", async () => {
    const props = await atReadyStep({}, { onOpen: mock(async () => false), error: "no such folder" });
    expect(screen.getByText("no such folder")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: /kibo-agent/ }));
    await waitFor(() => expect(props.folder.onOpen).toHaveBeenCalledTimes(1));
    expect(props.onFinish).not.toHaveBeenCalled();
  });

  test("Back from the last step keeps the form as it was saved", async () => {
    const props = atProviderStep();
    fireEvent.click(card(/OpenAI/));
    fireEvent.change(field(/API key/), { target: { value: "sk-kept" } });
    fireEvent.click(button("Continue"));
    await waitFor(() => expect(button("Connect")).toBeTruthy());
    fireEvent.click(button("Connect"));
    await waitFor(() => expect(screen.getByText("You're set")).toBeTruthy());

    fireEvent.click(button("Back"));
    // Not a blank form: an empty key deletes the one that was just stored.
    expect(field("Name").value).toBe("openai");
    expect(field(/API key/).value).toBe("sk-kept");

    fireEvent.click(button("Connect"));
    await waitFor(() => expect(screen.getByText("You're set")).toBeTruthy());
    expect((props.provider.onSave as ReturnType<typeof mock>).mock.calls[1][1]).toBe("sk-kept");
  });

  test("a picker that refuses says why and leaves the button usable", async () => {
    const folder = {
      onPick: mock(async () => {
        throw new Error("no permission to read folders");
      }),
    };
    const props = await atReadyStep({}, folder);
    fireEvent.click(button("Choose a folder…"));

    await waitFor(() => expect(screen.getByText(/no permission to read folders/)).toBeTruthy());
    expect(props.onFinish).not.toHaveBeenCalled();
    expect(button("Choose a folder…").disabled).toBe(false);
  });

  test("Start closes without a folder", async () => {
    const props = await atReadyStep();
    fireEvent.click(button("Start"));
    expect(props.onFinish).toHaveBeenCalledTimes(1);
  });
});
