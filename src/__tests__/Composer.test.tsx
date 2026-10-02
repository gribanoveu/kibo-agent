import { afterEach, describe, expect, mock, test } from "bun:test";
import { fireEvent, render, screen } from "@testing-library/react";

// The chip is the only way to reach `chat_set_mode`, and what it says has to
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

(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};

const { Composer } = await import("../components/Composer");
const { setConversationMode } = await import("../lib/chat");
const { modelChoices } = await import("../hooks/useLlmSettings");

afterEach(() => {
  calls.length = 0;
  refuse = false;
});

function composer(
  conversation: "agent" | "plan" | "ask",
  onConversation: (mode: "agent" | "plan" | "ask") => void = () => {},
  unattended = false,
  onUnattended: (unattended: boolean) => void = () => {},
) {
  return render(
    <Composer
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

describe("the conversation-mode chip", () => {
  test("offers each mode and reports the one that was picked", () => {
    const picked: string[] = [];
    composer("agent", (mode) => picked.push(mode));

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
    expect(screen.getByTitle("What the agent may do").textContent).toContain("Plan");
    expect(screen.getByTitle("What the agent may do").textContent).not.toContain("plan");
  });

  /// Two chips, two questions. Permissions decide who agrees before a call;
  /// the mode decides whether the call exists at all.
  test("does not replace the permission chip", () => {
    composer("agent");
    expect(screen.getByTitle("Permission mode")).toBeTruthy();
    expect(screen.getByTitle("What the agent may do")).toBeTruthy();
  });
});

describe("the permission chip", () => {
  /// It used to be three labels over a backend with two states, wired to
  /// nothing. A control that moves and changes nothing is worse than no
  /// control: it reads as a guarantee.
  test("offers exactly the two states the backend has", () => {
    composer("agent");
    fireEvent.click(screen.getByTitle("Permission mode"));

    const options = screen.getAllByRole("option").map((o) => o.textContent);
    expect(options).toEqual(["AskConfirm edits and commands", "AutoRun the whole turn without asking"]);
  });

  test("picking auto reports that nothing will be asked", () => {
    const picked: boolean[] = [];
    composer("agent", () => {}, false, (v) => picked.push(v));

    fireEvent.click(screen.getByTitle("Permission mode"));
    fireEvent.click(screen.getByRole("option", { name: /Auto/ }));
    expect(picked).toEqual([true]);
  });

  /// The label is the backend's state, not a click this component remembered.
  test("shows the state it was given", () => {
    composer("agent", () => {}, true);
    expect(screen.getByTitle("Permission mode").textContent).toContain("Auto");
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
    draft: { text: string; seq: number } | null,
    quote: { text: string; seq: number } | null = null,
  ) => (
    <Composer
      onSend={() => {}}
      onStop={() => {}}
      running={false}
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

  /// A branch gives its message back; the same text twice is two branches.
  test("replaces what was typed, each time it is handed", () => {
    const { rerender } = render(withDraft({ text: "try this", seq: 1 }));
    const box = screen.getByRole("textbox") as HTMLTextAreaElement;
    expect(box.value).toBe("try this");

    fireEvent.change(box, { target: { value: "edited" } });
    rerender(withDraft({ text: "try this", seq: 2 }));
    expect(box.value).toBe("try this");
  });

  /// A terminal selection is added to the message, not put in its place.
  test("a quote goes after what was typed, each time it is handed", () => {
    const { rerender } = render(withDraft(null, { text: "```\n1 failed\n```", seq: 1 }));
    const box = screen.getByRole("textbox") as HTMLTextAreaElement;
    expect(box.value).toBe("```\n1 failed\n```");

    fireEvent.change(box, { target: { value: "why did this fail?  " } });
    rerender(withDraft(null, { text: "```\n1 failed\n```", seq: 2 }));
    expect(box.value).toBe("why did this fail?\n\n```\n1 failed\n```");
  });
});

describe("the model chip", () => {
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

    expect(screen.getByTitle("Model").textContent).toContain("openrouter/anthropic/claude-sonnet-5");
    fireEvent.click(screen.getByTitle("Model"));
    expect(loads).toBe(1);
    fireEvent.click(screen.getByRole("option", { name: /openrouter\/openai\/gpt-5/ }));
    expect(picked).toEqual(["OpenRouter|openai/gpt-5"]);
  });

  test("the thinking chip shows the active provider's level and reports the pick", () => {
    const models = modelChoices(
      { ...settings, providers: settings.providers.map((p) => ({ ...p, reasoningEffort: "max" })) },
      {},
    );
    const picked: (string | null)[] = [];
    render(
      <Composer
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
    expect(ring.closest(".composer-bar")).toBeTruthy();
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

  /// Command files are edited outside the app: the list is asked for again
  /// each time the menu opens, not once at start-up.
  test("opening the menu asks for the list again, and shows what to type after a name", () => {
    let asked = 0;
    render(
      <Composer
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
    queued: { id: number; text: string }[] = [],
    steered: { id: string; text: string }[] = [],
  ) {
    const sent: string[] = [];
    const held: string[] = [];
    const taken: number[] = [];
    const withdrawn: string[] = [];
    const ran: string[] = [];
    render(
      <Composer
        onSend={(text) => sent.push(text)}
        onQueue={(text) => held.push(text)}
        queued={queued}
        onUnqueue={(id) => taken.push(id)}
        steered={steered}
        onWithdraw={(id) => withdrawn.push(id)}
        onStop={() => {}}
        running={running}
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
      />,
    );
    const box = screen.getByRole("textbox") as HTMLTextAreaElement;
    const altEnter = (text: string) => {
      fireEvent.change(box, { target: { value: text } });
      fireEvent.keyDown(box, { code: "Enter", key: "Enter", altKey: true });
    };
    return { sent, held, taken, withdrawn, ran, box, altEnter };
  }

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

  /// A note sent into the turn is listed above the queue — it is read sooner —
  /// and taking it back is its own action, not the queue's.
  test("a note the turn has not read yet is listed first, and can be taken back", () => {
    const { taken, withdrawn } = queueing(true, [{ id: 1, text: "then the docs" }], [{ id: "n-1", text: "use the helper" }]);
    const list = screen.getByRole("list", { name: "Queued messages" });
    expect([...list.querySelectorAll(".composer-queue-text")].map((el) => el.textContent)).toEqual(["use the helper", "then the docs"]);
    fireEvent.click(screen.getByRole("button", { name: "Take back to the message box" }));
    expect(withdrawn).toEqual(["n-1"]);
    expect(taken).toEqual([]);
  });
});
