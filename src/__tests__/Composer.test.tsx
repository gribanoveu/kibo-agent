import { afterEach, describe, expect, mock, test } from "bun:test";
import { fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import type { ImagePart } from "../lib/images";
import type { IndexState } from "../lib/indexStatus";

// The panel is the only way to reach `chat_set_mode`, and what it says has to
// be what the backend was actually told — a label reading "Plan" over a turn
// that is still armed is the failure worth a test.

const calls: { command: string; args: unknown }[] = [];
let refuse = false;

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args: unknown) => {
    calls.push({ command, args });
    return refuse ? Promise.reject(new Error("no")) : Promise.resolve(null);
  },
  transformCallback: (callback: unknown) => callback,
}));

// The box listens for files dropped on the window; nothing is dropped here.
mock.module("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ onDragDropEvent: () => Promise.resolve(() => {}) }),
}));

(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};

const { Composer } = await import("../components/Composer");
const { setConversationMode } = await import("../lib/chat");
const { modelChoices } = await import("../hooks/useLlmSettings");

afterEach(() => {
  calls.length = 0;
  refuse = false;
  localStorage.clear();
});

/** The panel the mode, the model and the switches live in. */
const openSettings = () => fireEvent.click(screen.getByTitle("Mode, model and thinking"));

function composer(
  conversation: "agent" | "plan" | "ask",
  onConversation: (mode: "agent" | "plan" | "ask") => void = () => {},
  unattended = false,
  onUnattended: (unattended: boolean) => void = () => {},
) {
  return render(
    <Composer
      draftKey="draft"
      onSend={() => {}}
      onStop={() => {}}
      running={false}
      conversation={conversation}
      onConversation={onConversation}
      unattended={unattended}
      onUnattended={onUnattended}
      models={{ choices: [], current: null }}
      onModel={() => {}}
      onEffort={() => {}}
      onLoadModels={() => {}}
      context={null}
      usage={null}
      onCompact={() => {}}
    />,
  );
}

describe("the settings panel", () => {
  /// Read before sending, without opening anything.
  test("says on one line what the message goes out with", () => {
    render(
      <Composer
        draftKey="draft"
        onSend={() => {}}
        onStop={() => {}}
        running={false}
        conversation="plan"
        onConversation={() => {}}
        unattended={false}
        onUnattended={() => {}}
        models={{
          choices: [],
          current: { providerId: "OpenRouter", model: "anthropic/claude-sonnet-5", label: "openrouter/anthropic/claude-sonnet-5" },
          effort: "high",
        }}
        onModel={() => {}}
        onEffort={() => {}}
        onLoadModels={() => {}}
        context={null}
        usage={null}
        onCompact={() => {}}
      />,
    );
    const words = [...document.querySelectorAll(".composer-settings-summary > span")].map((el) => el.textContent);
    expect(words).toEqual(["plan", "claude-sonnet-5", "high", "ask"]);
  });

  /// The app's words are lower case on the line; a model's name is its own.
  test("a model's name keeps its case on the line", () => {
    render(
      <Composer
        draftKey="draft"
        onSend={() => {}}
        onStop={() => {}}
        running={false}
        conversation="agent"
        onConversation={() => {}}
        unattended={false}
        onUnattended={() => {}}
        models={{ choices: [], current: { providerId: "Local", model: "GPT-4o", label: "local/GPT-4o" } }}
        onModel={() => {}}
        onEffort={() => {}}
        onLoadModels={() => {}}
        context={null}
        usage={null}
        onCompact={() => {}}
      />,
    );
    expect(document.querySelector(".composer-settings-shrink")?.textContent).toBe("GPT-4o");
  });

  test("with no model, the line says so and no level", () => {
    composer("agent");
    const words = [...document.querySelectorAll(".composer-settings-summary > span")].map((el) => el.textContent);
    expect(words).toEqual(["agent", "no model", "ask"]);
  });

  test("opens and closes on its button, and stays open while a row is used", () => {
    const picked: string[] = [];
    composer("agent", (mode) => picked.push(mode));
    expect(screen.queryByRole("dialog")).toBeNull();

    openSettings();
    fireEvent.click(screen.getByTitle("What the agent may do"));
    fireEvent.click(screen.getByRole("option", { name: /Plan/ }));
    expect(picked).toEqual(["plan"]);
    expect(screen.getByRole("dialog")).toBeTruthy();

    openSettings();
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  /// One key, one thing closed: the menu over the panel goes first.
  test("Escape closes a menu open in it first, then the panel", () => {
    composer("agent");
    openSettings();
    fireEvent.click(screen.getByTitle("What the agent may do"));
    expect(screen.getByRole("listbox")).toBeTruthy();

    fireEvent.keyDown(document.body, { code: "Escape", key: "Escape" });
    expect(screen.queryByRole("listbox")).toBeNull();
    expect(screen.getByRole("dialog")).toBeTruthy();

    fireEvent.keyDown(document.body, { code: "Escape", key: "Escape" });
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  test("a press outside it closes it, a press inside does not", () => {
    composer("agent");
    openSettings();
    fireEvent.pointerDown(screen.getByRole("dialog"));
    expect(screen.getByRole("dialog")).toBeTruthy();

    fireEvent.pointerDown(document.body);
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});

describe("the conversation mode", () => {
  test("offers each mode and reports the one that was picked", () => {
    const picked: string[] = [];
    composer("agent", (mode) => picked.push(mode));

    openSettings();
    fireEvent.click(screen.getByTitle("What the agent may do"));
    for (const label of ["Agent", "Plan", "Ask"]) {
      expect(screen.getByRole("option", { name: new RegExp(label) })).toBeTruthy();
    }

    fireEvent.click(screen.getByRole("option", { name: /Plan/ }));
    expect(picked).toEqual(["plan"]);
  });

  /// The wire value and the word on screen are different things: "plan" is
  /// what `domain::conversation_mode` deserializes, not a word for a menu.
  test("shows the word, not the wire value", () => {
    composer("plan");
    openSettings();
    expect(screen.getByTitle("What the agent may do").textContent).toContain("Plan");
    expect(screen.getByTitle("What the agent may do").textContent).not.toContain("plan");
  });

  /// Two controls, two questions. Permissions decide who agrees before a call;
  /// the mode decides whether the call exists at all.
  test("does not replace the permission switch", () => {
    composer("agent");
    openSettings();
    expect(screen.getByRole("switch", { name: "Approve automatically" })).toBeTruthy();
    expect(screen.getByTitle("What the agent may do")).toBeTruthy();
  });
});

describe("the permission switch", () => {
  test("turned on, reports that nothing will be asked", () => {
    const picked: boolean[] = [];
    composer("agent", () => {}, false, (v) => picked.push(v));

    openSettings();
    fireEvent.click(screen.getByRole("switch", { name: "Approve automatically" }));
    expect(picked).toEqual([true]);
  });

  /// Its word is part of the control: a click on it does what one on the switch does.
  test("a click on its name turns it too, once", () => {
    const picked: boolean[] = [];
    composer("agent", () => {}, true, (v) => picked.push(v));

    openSettings();
    fireEvent.click(screen.getByText("Approve automatically"));
    expect(picked).toEqual([false]);
  });

  /// What it shows is the backend's state, not a click this component remembered.
  test("shows the state it was given", () => {
    composer("agent", () => {}, true);
    openSettings();
    expect(screen.getByRole("switch", { name: "Approve automatically" }).getAttribute("aria-checked")).toBe("true");
  });

  /// Read before sending, with the panel closed — and Auto marked, since nobody will be asked.
  test("what it says is on the line, last, Auto marked and Ask not", () => {
    const first = composer("agent", () => {}, true);
    const words = () => [...document.querySelectorAll(".composer-settings-summary > span")];
    expect(words().at(-1)?.textContent).toBe("auto");
    expect(words().at(-1)?.className).toBe("composer-auto");
    first.unmount();

    composer("agent", () => {}, false);
    expect(words().at(-1)?.textContent).toBe("ask");
    expect(document.querySelector(".composer-auto")).toBeNull();
  });
});

describe("the worktree switch", () => {
  const box = (worktree?: { on: boolean; title: string; onChange: (on: boolean) => void }) =>
    render(
      <Composer
        draftKey="draft"
        onSend={() => {}}
        onStop={() => {}}
        running={false}
        conversation="agent"
        onConversation={() => {}}
        unattended={false}
        onUnattended={() => {}}
        worktree={worktree}
        models={{ choices: [], current: null }}
        onModel={() => {}}
        onEffort={() => {}}
        onLoadModels={() => {}}
        context={null}
        usage={null}
        onCompact={() => {}}
      />,
    );

  test("reports the change and says where the worktree would start", () => {
    const picked: boolean[] = [];
    box({ on: false, title: "Starts from main", onChange: (on) => picked.push(on) });
    openSettings();
    const toggle = screen.getByRole("switch", { name: "Worktree" });
    expect(toggle.getAttribute("aria-checked")).toBe("false");
    expect(toggle.closest("label")?.title).toBe("Starts from main");
    fireEvent.click(toggle);
    expect(picked).toEqual([true]);
  });

  /// A worktree is a new folder and a new chat in it: nothing a started chat can become.
  test("is not offered without being asked for", () => {
    box();
    openSettings();
    expect(screen.queryByRole("switch", { name: "Worktree" })).toBeNull();
  });
});

describe("the folder's index", () => {
  const box = (index: IndexState | null) =>
    render(
      <Composer
        index={index}
        draftKey="draft"
        onSend={() => {}}
        onStop={() => {}}
        running={false}
        conversation="agent"
        onConversation={() => {}}
        unattended={false}
        onUnattended={() => {}}
        models={{ choices: [], current: null }}
        onModel={() => {}}
        onEffort={() => {}}
        onLoadModels={() => {}}
        context={null}
        usage={null}
        onCompact={() => {}}
      />,
    );
  const ready: IndexState = { phase: "ready", done: 0, total: 0, embedded: 12, skipped: 0, embeddingError: null, error: null };

  /// It says little most of the time: it is in the panel, not on the line.
  test("is said in the settings panel, with its detail, and not on the line", () => {
    box(ready);
    expect(screen.queryByRole("status")).toBeNull();
    openSettings();
    const badge = screen.getByRole("status");
    expect(badge.textContent).toBe("Indexed");
    expect(badge.title).toContain("12 passages");
    expect(badge.closest(".composer-settings-panel")).toBeTruthy();
  });

  test("with nothing known, or no folder, the panel has no such row", () => {
    box(null);
    openSettings();
    expect(document.querySelector(".composer-settings-status")).toBeNull();
  });
});

describe("the line under the box", () => {
  test("holds the folder, the settings and the balance, in that order", () => {
    render(
      <Composer
        tab={<span>the folder</span>}
        end={<span>the balance</span>}
        draftKey="draft"
        onSend={() => {}}
        onStop={() => {}}
        running={false}
        conversation="agent"
        onConversation={() => {}}
        unattended={false}
        onUnattended={() => {}}
        models={{ choices: [], current: null }}
        onModel={() => {}}
        onEffort={() => {}}
        onLoadModels={() => {}}
        context={null}
        usage={null}
        onCompact={() => {}}
      />,
    );
    const strip = document.querySelector(".composer-strip");
    expect(strip?.previousElementSibling?.className).toBe("composer");
    const [folder, settings, balance] = [...(strip?.children ?? [])];
    expect(folder.textContent).toBe("the folder");
    expect(settings.className).toBe("composer-settings");
    expect(balance.textContent).toBe("the balance");
  });
});

describe("telling the backend", () => {
  test("the mode goes to the command that enforces it", async () => {
    await setConversationMode("ask");
    expect(calls).toEqual([{ command: "chat_set_mode", args: { mode: "ask" } }]);
  });

  /// A refusal has to reach the caller: the chip only moves after the backend
  /// agreed, and it can only do that if the failure is not swallowed here.
  test("a refused change is not reported as a success", async () => {
    refuse = true;
    expect(setConversationMode("plan")).rejects.toThrow();
  });
});

const { useBackendSetting } = await import("../hooks/useBackendSetting");
const { chatDraftKey, readChatDraft } = await import("../hooks/useChatDraft");
const { act, renderHook } = await import("@testing-library/react");

describe("holding the chosen mode", () => {
  /// The chip must not move ahead of the backend. Showing "Plan" over a turn
  /// that is still armed is worse than showing the old mode for a moment.
  test("the mode only changes once the backend has agreed", async () => {
    refuse = true;
    const { result } = renderHook(() => useBackendSetting(setConversationMode, "agent"));

    let failed: string | null = null;
    await act(async () => {
      failed = await result.current.pick("plan");
    });

    expect(failed).toBeTruthy();
    expect(result.current.value).toBe("agent");
  });

  test("a change the backend accepted is kept", async () => {
    const { result } = renderHook(() => useBackendSetting(setConversationMode, "agent"));

    await act(async () => {
      expect(await result.current.pick("ask")).toBeNull();
    });

    expect(result.current.value).toBe("ask");
    expect(calls).toEqual([{ command: "chat_set_mode", args: { mode: "ask" } }]);
  });

  /// The chip the prototype drew and nobody connected. The command existed on
  /// the backend for two stages with no way to reach it.
  test("the permission chip reaches the policy that enforces it", async () => {
    const { setUnattended } = await import("../lib/chat");
    await setUnattended(true, "c1");
    expect(calls).toEqual([
      { command: "approval_set_unattended", args: { unattended: true, chatId: "c1" } },
    ]);
  });
});

describe("text handed to the box", () => {
  const withDraft = (
    draft: { text: string; seq: number; images?: ImagePart[] } | null,
    quote: { text: string; seq: number; images?: ImagePart[] } | null = null,
    imagesEnabled = true,
  ) => (
    <Composer
      draftKey="draft"
      onSend={() => {}}
      onStop={() => {}}
      running={false}
      imagesEnabled={imagesEnabled}
      conversation="agent"
      onConversation={() => {}}
      unattended={false}
      onUnattended={() => {}}
      draft={draft}
      quote={quote}
      models={{ choices: [], current: null }}
      onModel={() => {}}
      onEffort={() => {}}
      onLoadModels={() => {}}
      context={null}
      usage={null}
      onCompact={() => {}}
    />
  );

  /// A draft put back into a chat that already holds its pictures puts them
  /// back rather than adding a second copy — the draft stays as it is, so it
  /// arrives again each time the chat is opened.
  test("a draft's pictures land once, however often it arrives", () => {
    const picture: ImagePart = { mediaType: "image/png", data: "PNG", width: 10, height: 5 };
    const { rerender } = render(withDraft(null));
    rerender(withDraft({ text: "look", seq: 1, images: [picture] }));
    expect(screen.getAllByRole("img")).toHaveLength(1);

    rerender(withDraft({ text: "look", seq: 2, images: [picture] }));
    expect(screen.getAllByRole("img")).toHaveLength(1);
  });

  /// A draft is the whole of what was there, pictures included: one without
  /// any does not leave the box's behind to ride with its message.
  test("a draft without pictures empties the box", () => {
    const picture: ImagePart = { mediaType: "image/png", data: "PNG", width: 4, height: 3 };
    const { rerender } = render(withDraft(null));
    rerender(withDraft(null, { text: "look", seq: 1, images: [picture] }));
    expect(screen.getAllByRole("img")).toHaveLength(1);

    rerender(withDraft({ text: "try again", seq: 2 }));
    expect(screen.queryAllByRole("img")).toHaveLength(0);
  });

  /// A note taken out of a running turn comes home with its picture: the
  /// words were never sent, so neither were the pixels.
  test("pictures come back with the text a note was taken back with", () => {
    const picture: ImagePart = { mediaType: "image/png", data: "PNG", width: 10, height: 5 };
    const { rerender } = render(withDraft(null));
    rerender(withDraft(null, { text: "look at this", seq: 1, images: [picture] }));
    expect(screen.getByAltText("Image 1, 10×5")).toBeTruthy();
  });

  /// The box takes no pictures from a model set to text only, and a note
  /// coming back is no exception: it says where to turn them on instead.
  test("pictures handed back to a text-only model are refused, with the reason", () => {
    const picture: ImagePart = { mediaType: "image/png", data: "PNG", width: 10, height: 5 };
    const { rerender } = render(withDraft(null));
    rerender(withDraft(null, { text: "", seq: 1, images: [picture] }, false));
    expect(screen.getByRole("alert").textContent).toContain("Settings → Models");
    expect(screen.queryAllByRole("img")).toHaveLength(0);
  });

  /// The box holds what one message can carry. What comes back over that is
  /// said — the message it came from has already been cleared, so it cannot
  /// be left out in silence.
  test("pictures that do not fit are said, not dropped without a word", () => {
    const ten: ImagePart[] = Array.from({ length: 10 }, (_, n) => ({ mediaType: "image/png", data: `P${n}`, width: 1, height: 1 }));
    const two: ImagePart[] = Array.from({ length: 2 }, (_, n) => ({ mediaType: "image/png", data: `Q${n}`, width: 1, height: 1 }));
    const { rerender } = render(withDraft(null));
    rerender(withDraft(null, { text: "", seq: 1, images: ten }));
    expect(screen.getAllByRole("img")).toHaveLength(10);

    rerender(withDraft(null, { text: "", seq: 2, images: two }));
    expect(screen.getAllByRole("img")).toHaveLength(10);
    expect(screen.getByRole("alert").textContent).toContain("2 pictures did not fit");
  });

  /// A branch gives its message back; the same text twice is two branches.
  test("replaces what was typed, each time it is handed", () => {
    const { rerender } = render(withDraft(null));
    rerender(withDraft({ text: "try this", seq: 1 }));
    const box = screen.getByRole("textbox") as HTMLTextAreaElement;
    expect(box.value).toBe("try this");

    fireEvent.change(box, { target: { value: "edited" } });
    rerender(withDraft({ text: "try this", seq: 2 }));
    expect(box.value).toBe("try this");
  });

  /// The box is drawn anew when the mode changes. What was handed before is
  /// not applied again — the user may have typed over it since — and what it
  /// was left holding is the draft it comes back with.
  test("a box drawn anew keeps what was left in it, not the text handed before", () => {
    const draft = { text: "try this", seq: 1 };
    const quote = { text: "1 failed", seq: 1 };
    const first = render(withDraft(null));
    first.rerender(withDraft(draft, quote));
    fireEvent.change(screen.getByRole("textbox"), { target: { value: "edited" } });
    first.unmount();

    render(withDraft(draft, quote));
    expect((screen.getByRole("textbox") as HTMLTextAreaElement).value).toBe("edited");
  });

  /// A terminal selection is added to the message, not put in its place.
  test("a quote goes after what was typed, each time it is handed", () => {
    const { rerender } = render(withDraft(null));
    rerender(withDraft(null, { text: "```\n1 failed\n```", seq: 1 }));
    const box = screen.getByRole("textbox") as HTMLTextAreaElement;
    expect(box.value).toBe("```\n1 failed\n```");

    fireEvent.change(box, { target: { value: "why did this fail?  " } });
    rerender(withDraft(null, { text: "```\n1 failed\n```", seq: 2 }));
    expect(box.value).toBe("why did this fail?\n\n```\n1 failed\n```");
  });
});

describe("the model row", () => {
  const settings = {
    providers: [
      { id: "OpenRouter", baseUrl: "https://openrouter.ai/api/v1", model: "Anthropic/Claude-Sonnet-5", hasApiKey: true },
      { id: "local", baseUrl: "http://127.0.0.1:1234/v1", hasApiKey: false },
    ],
    activeProviderId: "OpenRouter",
    debugLogging: false,
  };

  test("offers provider/model in lower case, the pinned one current, auto for none pinned", () => {
    const { choices, current } = modelChoices(settings, { OpenRouter: ["Anthropic/Claude-Sonnet-5", "openai/gpt-5"] });
    expect(choices.map((c) => c.label)).toEqual([
      "openrouter/anthropic/claude-sonnet-5",
      "openrouter/openai/gpt-5",
      "local/auto",
    ]);
    expect(current?.label).toBe("openrouter/anthropic/claude-sonnet-5");
  });

  test("shows the current one, asks for the list on open, and reports the pick", () => {
    const models = modelChoices(settings, { OpenRouter: ["openai/gpt-5"] });
    const picked: string[] = [];
    let loads = 0;
    render(
      <Composer
        draftKey="draft"
        onSend={() => {}}
        onStop={() => {}}
        running={false}
        conversation="agent"
        onConversation={() => {}}
        unattended={false}
        onUnattended={() => {}}
        models={models}
        onModel={(c) => picked.push(`${c.providerId}|${c.model}`)}
        onEffort={() => {}}
        onLoadModels={() => loads++}
        context={{ instructions: 1_000, tools: 3_000, conversation: 4_000, total: 8_000, limit: 200_000, compactsAt: null }}
        usage={null}
        onCompact={() => {}}
      />,
    );

    openSettings();
    expect(screen.getByTitle("Model").textContent).toContain("openrouter/anthropic/claude-sonnet-5");
    fireEvent.click(screen.getByTitle("Model"));
    expect(loads).toBe(1);
    fireEvent.click(screen.getByRole("option", { name: /openrouter\/openai\/gpt-5/ }));
    expect(picked).toEqual(["OpenRouter|openai/gpt-5"]);
  });

  test("the thinking row shows the active provider's level and reports the pick", () => {
    const models = modelChoices(
      { ...settings, providers: settings.providers.map((p) => ({ ...p, reasoningEffort: "max" })) },
      {},
    );
    const picked: (string | null)[] = [];
    render(
      <Composer
        draftKey="draft"
        onSend={() => {}}
        onStop={() => {}}
        running={false}
        conversation="agent"
        onConversation={() => {}}
        unattended={false}
        onUnattended={() => {}}
        models={models}
        onModel={() => {}}
        onEffort={(e) => picked.push(e)}
        onLoadModels={() => {}}
        context={null}
        usage={null}
        onCompact={() => {}}
      />,
    );

    openSettings();
    // A level typed by hand in Settings is still the one shown as chosen.
    expect(screen.getByTitle("Thinking level").textContent).toContain("max");
    fireEvent.click(screen.getByTitle("Thinking level"));
    fireEvent.click(screen.getByRole("option", { name: /High/ }));
    fireEvent.click(screen.getByTitle("Thinking level"));
    fireEvent.click(screen.getByRole("option", { name: /Default/ }));
    expect(picked).toEqual(["high", null]);
  });

  test("the context ring sits in the bar, beside the send button", () => {
    render(
      <Composer
        draftKey="draft"
        onSend={() => {}}
        onStop={() => {}}
        running={false}
        conversation="agent"
        onConversation={() => {}}
        unattended={false}
        onUnattended={() => {}}
        models={{ choices: [], current: null }}
        onModel={() => {}}
        onEffort={() => {}}
        onEffort={() => {}}
      onLoadModels={() => {}}
        context={{ instructions: 1_000, tools: 3_000, conversation: 4_000, total: 8_000, limit: 200_000, compactsAt: null }}
        usage={null}
        onCompact={() => {}}
      />,
    );
    const ring = screen.getByRole("button", { name: "Context usage: 4%" });
    expect(ring.closest(".composer-meter")?.nextElementSibling?.className).toContain("send");
  });
});

describe("slash commands", () => {
  function withCommands(unavailable?: string, withArguments = false, pending?: string) {
    const ran: string[] = [];
    const sent: string[] = [];
    const commands = [
      { name: "compact", hint: "Fold older history", run: (args: string) => ran.push(`compact:${args}`) },
      { name: "fork", hint: "Copy the chat", unavailable, run: (args: string) => ran.push(`fork:${args}`) },
      ...(withArguments
        ? [{ name: "compare", hint: "Compare two files", argumentHint: "<old> <new>", run: (args: string) => ran.push(`compare:${args}`) }]
        : []),
    ];
    render(
      <Composer
        draftKey="draft"
        onSend={(text) => sent.push(text)}
        onStop={() => {}}
        running={false}
        conversation="agent"
        onConversation={() => {}}
        unattended={false}
        onUnattended={() => {}}
        models={{ choices: [], current: null }}
        onModel={() => {}}
        onEffort={() => {}}
        onLoadModels={() => {}}
        context={null}
        usage={null}
        onCompact={() => {}}
        commands={commands}
        pendingCommands={pending}
      />,
    );
    const box = screen.getByRole("textbox") as HTMLTextAreaElement;
    const type = (text: string) => fireEvent.change(box, { target: { value: text } });
    const key = (code: string, k = code) => fireEvent.keyDown(box, { code, key: k });
    return { ran, sent, box, type, key };
  }

  test("a slash offers every command, and typing narrows them", () => {
    const { type } = withCommands();
    type("/");
    expect(screen.getAllByRole("option").map((o) => o.querySelector(".slash-name")?.textContent)).toEqual([
      "/compact",
      "/fork",
    ]);
    type("/f");
    expect(screen.getAllByRole("option").map((o) => o.querySelector(".slash-name")?.textContent)).toEqual(["/fork"]);
    type("/f ");
    expect(screen.queryByRole("listbox")).toBeNull();
  });

  test("Enter runs the highlighted command instead of sending it, and the arrows move", () => {
    const { ran, sent, box, type, key } = withCommands();
    type("/");
    key("ArrowDown");
    key("Enter");
    expect(ran).toEqual(["fork:"]);
    expect(sent).toEqual([]);
    expect(box.value).toBe("");
  });

  test("a typed command with arguments runs with them", () => {
    const { ran, type, key } = withCommands();
    type("/compact keep the plan");
    key("Enter");
    expect(ran).toEqual(["compact:keep the plan"]);
  });

  test("a click on a command runs it", () => {
    const { ran, type } = withCommands();
    type("/");
    fireEvent.click(screen.getByRole("option", { name: /compact/ }));
    expect(ran).toEqual(["compact:"]);
  });

  /// Run bare, a command that wants arguments would send its placeholders:
  /// Enter or a click puts it in the box, and Enter then runs what was typed.
  test("a command that takes arguments is put in the box, not run", () => {
    const { ran, box, type, key } = withCommands(undefined, true);
    type("/compar");
    expect(document.querySelector(".slash-keys")?.textContent).toBe("Tab or Enter to fill it in, then type the arguments");
    key("Enter");
    expect(box.value).toBe("/compare ");
    expect(ran).toEqual([]);

    type("/compare a.ts b.ts");
    key("Enter");
    expect(ran).toEqual(["compare:a.ts b.ts"]);

    type("/");
    fireEvent.click(screen.getByRole("option", { name: /compare/ }));
    expect(box.value).toBe("/compare ");
    expect(ran).toEqual(["compare:a.ts b.ts"]);
  });

  test("the menu says what the keys do with the highlighted command", () => {
    const { type, key } = withCommands(undefined, true);
    type("/");
    expect(document.querySelector(".slash-keys")?.textContent).toBe("Enter to run · Tab to fill it in");
    key("ArrowUp");
    expect(document.querySelector(".slash-keys")?.textContent).toBe("Tab or Enter to fill it in, then type the arguments");
  });

  test("Tab completes the name and leaves room for arguments", () => {
    const { ran, box, type, key } = withCommands();
    type("/co");
    key("Tab");
    expect(box.value).toBe("/compact ");
    expect(ran).toEqual([]);
  });

  /// `/tmp is full` is a message about a folder, not a command nobody wrote.
  test("text with a slash but no such command is sent as it is", () => {
    const { ran, sent, type, key } = withCommands();
    type("/tmp is full");
    key("Enter");
    expect(sent).toEqual(["/tmp is full"]);
    expect(ran).toEqual([]);
  });

  test("Escape closes the menu, and Enter then sends what is typed", () => {
    const { ran, sent, type, key } = withCommands();
    type("/x");
    expect(screen.queryByRole("listbox")).toBeNull();
    type("/");
    key("Escape");
    expect(screen.queryByRole("listbox")).toBeNull();
    type("/compact");
    expect(screen.getByRole("listbox")).toBeTruthy();
    key("Escape");
    key("Enter");
    // A command's name is still a command with the menu closed.
    expect(ran).toEqual(["compact:"]);
    expect(sent).toEqual([]);
  });

  test("Enter sends nothing while an IME composes, or with a modifier held", () => {
    const { sent, box, type } = withCommands();
    type("こんにちは");
    fireEvent.keyDown(box, { code: "Enter", key: "Enter", isComposing: true });
    fireEvent.keyDown(box, { code: "Enter", key: "Enter", metaKey: true });
    fireEvent.keyDown(box, { code: "Enter", key: "Enter", ctrlKey: true });
    expect(sent).toEqual([]);
    fireEvent.keyDown(box, { code: "Enter", key: "Enter" });
    expect(sent).toEqual(["こんにちは"]);
  });

  /// Command files are edited outside the app: the list is asked for again
  /// each time the menu opens, not once at start-up.
  test("opening the menu asks for the list again, and shows what to type after a name", () => {
    let asked = 0;
    render(
      <Composer
        draftKey="draft"
        onSend={() => {}}
        onStop={() => {}}
        running={false}
        conversation="agent"
        onConversation={() => {}}
        unattended={false}
        onUnattended={() => {}}
        models={{ choices: [], current: null }}
        onModel={() => {}}
        onEffort={() => {}}
        onLoadModels={() => {}}
        context={null}
        usage={null}
        onCompact={() => {}}
        commands={[{ name: "review", hint: "Review a file", argumentHint: "<file>", run: () => {} }]}
        onCommandsOpen={() => asked++}
      />,
    );
    const box = screen.getByRole("textbox");
    fireEvent.change(box, { target: { value: "/" } });
    fireEvent.change(box, { target: { value: "/re" } });
    expect(asked).toBe(1);
    expect(screen.getByRole("option").textContent).toContain("/review <file>");

    // Picked, the arguments still to type show in grey after the text, and
    // go as they are typed.
    fireEvent.change(box, { target: { value: "/review " } });
    expect(document.querySelector(".composer-ghost-hint")?.textContent).toBe("<file>");
    fireEvent.change(box, { target: { value: "/review src/a.ts" } });
    expect(document.querySelector(".composer-ghost")).toBeNull();

    fireEvent.change(box, { target: { value: "" } });
    fireEvent.change(box, { target: { value: "/" } });
    expect(asked).toBe(2);
  });

  /// It stays in the box, and the menu says why, rather than vanishing.
  test("a command that cannot run now says why and keeps the text", () => {
    const { ran, box, type, key } = withCommands("Nothing to fork yet");
    type("/fork");
    expect(screen.getByRole("option").textContent).toContain("Nothing to fork yet");
    key("Enter");
    expect(ran).toEqual([]);
    expect(box.value).toBe("/fork");
  });

  /// The servers' prompts are on their way: the menu says so under what it
  /// has, and stays open for a name none of those matches — it may be one of
  /// the prompts.
  test("while more commands are coming the menu says so, and stays open for a name nothing matches yet", () => {
    const { type } = withCommands(undefined, false, "Loading MCP prompts…");
    type("/");
    expect(screen.getByRole("status").textContent).toBe("Loading MCP prompts…");
    expect(screen.getAllByRole("option")).toHaveLength(2);

    type("/gre");
    expect(screen.queryAllByRole("option")).toEqual([]);
    expect(screen.getByRole("status").textContent).toBe("Loading MCP prompts…");

    type("/gre who");
    expect(screen.queryByRole("status")).toBeNull();
  });

  test("with nothing coming, a name nothing matches closes the menu", () => {
    const { type } = withCommands();
    type("/gre");
    expect(screen.queryByRole("listbox")).toBeNull();
  });

  /// Nothing to pick while only the line is shown: the keys do what they do
  /// with the menu closed.
  test("Enter with only the waiting line sends the text, and the arrows move nothing", () => {
    const { type, key, sent, ran } = withCommands(undefined, false, "Loading MCP prompts…");
    type("/gre");
    key("ArrowDown");
    key("Tab");
    key("Enter");
    expect(sent).toEqual(["/gre"]);
    expect(ran).toEqual([]);
  });
});

describe("the box's focus", () => {
  const withFocus = (focus: number) => (
    <Composer
      draftKey="draft"
      onSend={() => {}}
      onStop={() => {}}
      running={false}
      conversation="agent"
      onConversation={() => {}}
      unattended={false}
      onUnattended={() => {}}
      models={{ choices: [], current: null }}
      onModel={() => {}}
      onEffort={() => {}}
      onLoadModels={() => {}}
      context={null}
      usage={null}
      onCompact={() => {}}
      focus={focus}
    />
  );

  /// ⌘L and a new chat put the cursor in the box, from wherever it was.
  test("is taken each time it is asked for, and not before", () => {
    const { rerender } = render(withFocus(0));
    const box = screen.getByRole("textbox");
    expect(document.activeElement).not.toBe(box);

    rerender(withFocus(1));
    expect(document.activeElement).toBe(box);

    box.blur();
    rerender(withFocus(2));
    expect(document.activeElement).toBe(box);
  });
});

describe("the queue", () => {
  function queueing(
    running: boolean,
    queued: { id: number; text: string; images?: ImagePart[] }[] = [],
  ) {
    const sent: string[] = [];
    const held: string[] = [];
    const heldPictures: ImagePart[][] = [];
    const taken: number[] = [];
    const ran: string[] = [];
    // The box's whole state, so a test can hand it a picture the way a
    // taken-back note does — through `quote`, which only new values apply.
    const build = (quote: { text: string; seq: number; images?: ImagePart[] } | null = null) => (
      <Composer
        draftKey="draft"
        onSend={(text) => sent.push(text)}
        onQueue={(text, images) => {
          held.push(text);
          heldPictures.push(images);
        }}
        queued={queued}
        onUnqueue={(id) => taken.push(id)}
        onStop={() => {}}
        running={running}
        imagesEnabled
        quote={quote}
        conversation="agent"
        onConversation={() => {}}
        unattended={false}
        onUnattended={() => {}}
        models={{ choices: [], current: null }}
        onModel={() => {}}
        onEffort={() => {}}
        onLoadModels={() => {}}
        context={null}
        usage={null}
        onCompact={() => {}}
        commands={[{ name: "compact", hint: "Fold", run: (args: string) => ran.push(args) }]}
      />
    );
    const view = render(build());
    const box = screen.getByRole("textbox") as HTMLTextAreaElement;
    const altEnter = (text: string) => {
      fireEvent.change(box, { target: { value: text } });
      fireEvent.keyDown(box, { code: "Enter", key: "Enter", altKey: true });
    };
    return { ...view, build, sent, held, heldPictures, taken, ran, box, altEnter };
  }

  /// A message held for after the turn carries its pictures too: left in the
  /// box they would ride along with whatever is typed next.
  test("a held message takes its pictures with it", () => {
    const picture: ImagePart = { mediaType: "image/png", data: "PNG", width: 4, height: 3 };
    const { build, rerender, held, heldPictures, altEnter } = queueing(true);
    rerender(build({ text: "", seq: 1, images: [picture] }));
    expect(screen.getByAltText("Image 1, 4×3")).toBeTruthy();

    altEnter("look at this");
    expect(held).toEqual(["look at this"]);
    expect(heldPictures).toEqual([[picture]]);
    expect(screen.queryAllByRole("img")).toHaveLength(0);
  });

  /// With nothing typed around it, the held message is the picture.
  test("a picture alone can be held for after the turn", () => {
    const picture: ImagePart = { mediaType: "image/png", data: "PNG", width: 4, height: 3 };
    const { build, rerender, held, heldPictures, altEnter } = queueing(true);
    rerender(build({ text: "", seq: 1, images: [picture] }));

    altEnter("");
    expect(held).toEqual([""]);
    expect(heldPictures).toEqual([[picture]]);
  });

  test("Alt+Enter during a turn holds the message instead of steering with it", () => {
    const { sent, held, box, altEnter } = queueing(true);
    altEnter("then update the docs");
    expect(held).toEqual(["then update the docs"]);
    expect(sent).toEqual([]);
    expect(box.value).toBe("");
  });

  test("with nothing running there is nothing to wait for: it is sent", () => {
    const { sent, held, altEnter } = queueing(false);
    altEnter("hello");
    expect(sent).toEqual(["hello"]);
    expect(held).toEqual([]);
  });

  test("a command is run, not queued", () => {
    const { held, ran, altEnter } = queueing(true);
    altEnter("/compact now");
    expect(ran).toEqual(["now"]);
    expect(held).toEqual([]);
  });

  test("what waits is listed, and a row can be taken back", () => {
    const { taken } = queueing(true, [
      { id: 1, text: "first" },
      { id: 2, text: "second" },
    ]);
    const list = screen.getByRole("list", { name: "Queued messages" });
    expect([...list.querySelectorAll(".composer-queue-text")].map((el) => el.textContent)).toEqual(["first", "second"]);
    fireEvent.click(screen.getAllByRole("button", { name: "Back to the message box" })[1]);
    expect(taken).toEqual([2]);
  });

  /// A message can be a picture with nothing typed around it: the row has to
  /// say something, and what it says is that it is a picture.
  test("a message that is only a picture is listed as one", () => {
    const picture: ImagePart = { mediaType: "image/png", data: "PNG", width: 4, height: 3 };
    queueing(true, [{ id: 1, text: "", images: [picture] }]);
    expect(screen.getByRole("button", { name: "A picture" })).toBeTruthy();
  });

  test("a message of several pictures says how many", () => {
    const picture: ImagePart = { mediaType: "image/png", data: "PNG", width: 4, height: 3 };
    const two: ImagePart = { mediaType: "image/png", data: "PNG", width: 5, height: 3 };
    queueing(true, [{ id: 1, text: "", images: [picture, two] }]);
    expect(screen.getByRole("button", { name: "2 pictures" })).toBeTruthy();
  });

  /// What was typed stays first and the picture is said after it, in a note
  /// and in a held message alike.
  test("a row with words says it carries a picture too", () => {
    const picture: ImagePart = { mediaType: "image/png", data: "PNG", width: 4, height: 3 };
    queueing(true, [{ id: 1, text: "then the docs", images: [picture] }]);
    expect(screen.getByRole("button", { name: "then the docs · a picture" })).toBeTruthy();
  });

  /// What a row is and when it is read is said on it, not in a tooltip; a long
  /// one opens whole on a click, one at a time.
  test("a row says what it is and when it is read, and opens whole on a click", () => {
    queueing(true, [
      { id: 1, text: "use the helper" },
      { id: 2, text: "then the docs" },
    ]);
    const list = screen.getByRole("list", { name: "Queued messages" });
    expect([...list.querySelectorAll(".composer-queue-kind")].map((el) => el.textContent)).toEqual(["Queued", "Queued"]);
    expect([...list.querySelectorAll(".composer-queue-when")].map((el) => el.textContent)).toEqual(["After this turn", "After this turn"]);

    const [note, queued] = [screen.getByRole("button", { name: "use the helper" }), screen.getByRole("button", { name: "then the docs" })];
    fireEvent.click(note);
    expect(note.getAttribute("aria-expanded")).toBe("true");
    fireEvent.click(queued);
    expect([note.getAttribute("aria-expanded"), queued.getAttribute("aria-expanded")]).toEqual(["false", "true"]);
    fireEvent.click(queued);
    expect(queued.getAttribute("aria-expanded")).toBe("false");
  });
});

describe("the draft a chat keeps", () => {
  const boxFor = (key: string, onSend: (text: string) => void = () => {}) => (
    <Composer
      draftKey={key}
      onSend={onSend}
      onStop={() => {}}
      running={false}
      conversation="agent"
      onConversation={() => {}}
      unattended={false}
      onUnattended={() => {}}
      models={{ choices: [], current: null }}
      onModel={() => {}}
      onEffort={() => {}}
      onLoadModels={() => {}}
      context={null}
      usage={null}
      onCompact={() => {}}
    />
  );
  const type = (text: string) => fireEvent.change(screen.getByRole("textbox"), { target: { value: text } });
  const value = () => (screen.getByRole("textbox") as HTMLTextAreaElement).value;

  /// The box belongs to a chat, not to the window: what was typed in one waits
  /// there, and the next chat starts with its own text — nothing or its own.
  test("each chat finds its own text, and only its own", () => {
    const a = chatDraftKey("agent", "a");
    const b = chatDraftKey("agent", "b");
    const { rerender } = render(boxFor(a));

    type("про первый");
    rerender(boxFor(b));
    expect(value()).toBe("");

    type("про второй");
    rerender(boxFor(a));
    expect(value()).toBe("про первый");
    rerender(boxFor(b));
    expect(value()).toBe("про второй");
  });

  /// A chat not saved yet is one slot per mode: what was typed in a new chat is
  /// in the next new one, and not in a chat opened from the list.
  test("a chat opened from the list does not take the new chat's text", () => {
    const { rerender } = render(boxFor(chatDraftKey("agent", null)));
    type("новый чат");
    rerender(boxFor(chatDraftKey("agent", "a")));
    expect(value()).toBe("");
    rerender(boxFor(chatDraftKey("agent", null)));
    expect(value()).toBe("новый чат");
  });

  /// Sent, the text is the turn's: nothing is kept for the next visit.
  test("a message sent leaves no draft behind", () => {
    const sent: string[] = [];
    const key = chatDraftKey("agent", "a");
    const { rerender } = render(boxFor(key, (text) => sent.push(text)));
    type("сделай");
    expect(readChatDraft(key)).toBe("сделай");
    fireEvent.keyDown(screen.getByRole("textbox"), { code: "Enter", key: "Enter" });
    expect(sent).toEqual(["сделай"]);
    expect(readChatDraft(key)).toBe("");

    rerender(boxFor(chatDraftKey("agent", "b")));
    rerender(boxFor(key));
    expect(value()).toBe("");
  });

  /// A draft is not being typed: one that starts like a command opens no menu
  /// where the box appears, and asks for no list of commands, until a key is
  /// pressed — the same as a menu closed with Escape.
  test("a restored draft does not open the command menu by itself", () => {
    let asked = 0;
    const key = chatDraftKey("agent", "a");
    localStorage.setItem(key, "/rev");
    render(
      <Composer
        draftKey={key}
        onSend={() => {}}
        onStop={() => {}}
        running={false}
        conversation="agent"
        onConversation={() => {}}
        unattended={false}
        onUnattended={() => {}}
        models={{ choices: [], current: null }}
        onModel={() => {}}
        onEffort={() => {}}
        onLoadModels={() => {}}
        context={null}
        usage={null}
        onCompact={() => {}}
        commands={[{ name: "review", hint: "Review a file", run: () => {} }]}
        onCommandsOpen={() => asked++}
      />,
    );
    expect(value()).toBe("/rev");
    expect(screen.queryByRole("listbox")).toBeNull();
    expect(asked).toBe(0);

    type("/revi");
    expect(screen.getByRole("listbox")).toBeTruthy();
    expect(asked).toBe(1);
  });

  /// A command can start another chat — `/fork` does. What was in the box
  /// belongs to the chat it was typed in, and the box it moves to is not that
  /// one, so the draft goes with the message the command sent.
  test("a command that starts another chat leaves no draft behind", () => {
    const first = chatDraftKey("agent", "a");
    function Forking() {
      const [key, setKey] = useState(first);
      return (
        <Composer
          draftKey={key}
          onSend={() => {}}
          onStop={() => {}}
          running={false}
          conversation="agent"
          onConversation={() => {}}
          unattended={false}
          onUnattended={() => {}}
          models={{ choices: [], current: null }}
          onModel={() => {}}
          onEffort={() => {}}
          onLoadModels={() => {}}
          context={null}
          usage={null}
          onCompact={() => {}}
          commands={[{ name: "fork", hint: "Copy the chat", run: () => setKey(chatDraftKey("agent", null)) }]}
        />
      );
    }
    render(<Forking />);
    type("/fork");
    expect(readChatDraft(first)).toBe("/fork");

    fireEvent.keyDown(screen.getByRole("textbox"), { code: "Enter", key: "Enter" });
    expect(readChatDraft(first)).toBe("");
    expect(value()).toBe("");
  });
});
