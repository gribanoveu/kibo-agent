import { beforeEach, describe, expect, test } from "bun:test";
import { dictationStep, holdKey, HOLD_MS, insertPhrase, micButton, NOT_DICTATING, WAVE_BARS, type DictationAction, type DictationView } from "../lib/dictation";
import { comboKeys, matches, released, shortcutText, SHORTCUTS } from "../lib/shortcuts";

const after = (actions: DictationAction[], from: DictationView = NOT_DICTATING) => actions.reduce(dictationStep, from);

describe("a dictation's state", () => {
  test("goes from preparing through listening to its end", () => {
    expect(after([{ kind: "preparing" }]).phase).toBe("preparing");
    expect(after([{ kind: "preparing" }, { kind: "listening" }]).phase).toBe("listening");
    expect(after([{ kind: "preparing" }, { kind: "listening" }, { kind: "stopping" }]).phase).toBe("finishing");
    expect(after([{ kind: "preparing" }, { kind: "listening" }, { kind: "stopping" }, { kind: "ended", reason: "stopped" }])).toEqual(NOT_DICTATING);
  });

  test("a first buffer after Stop does not bring listening back", () => {
    expect(after([{ kind: "preparing" }, { kind: "stopping" }, { kind: "listening" }]).phase).toBe("finishing");
  });

  test("a Stop with nothing running is nothing", () => {
    expect(after([{ kind: "stopping" }])).toEqual(NOT_DICTATING);
  });

  test("a new one starts clean", () => {
    const before: DictationView = { phase: "idle", partial: "старое", peaks: [0.5], error: "сломалось" };
    expect(after([{ kind: "preparing" }], before)).toEqual({ ...NOT_DICTATING, phase: "preparing" });
  });

  test("the phrase being said lasts until it is whole", () => {
    const said = after([{ kind: "preparing" }, { kind: "partial", text: "при" }, { kind: "partial", text: "привет" }]);
    expect(said.partial).toBe("привет");
    expect(dictationStep(said, { kind: "phrase", text: "Привет." }).partial).toBe("");
  });

  test("the wave keeps its newest peaks, in order", () => {
    const level = (p: number) => ({ kind: "levels", peaks: [p, p, p, p] }) as DictationAction;
    const heard = after(Array.from({ length: WAVE_BARS / 4 + 1 }, (_, i) => level(i / 100)));
    expect(heard.peaks.length).toBe(WAVE_BARS);
    expect(heard.peaks.at(-1)).toBe(WAVE_BARS / 4 / 100);
    expect(heard.peaks[0]).toBe(1 / 100);
  });

  test("a failure or a refusal ends it and says why until closed", () => {
    for (const action of [{ kind: "failed", message: "нет" }, { kind: "refused", message: "нет" }] as DictationAction[]) {
      const failed = after([{ kind: "preparing" }, { kind: "partial", text: "при" }, action]);
      expect(failed).toEqual({ ...NOT_DICTATING, error: "нет" });
      expect(dictationStep(failed, { kind: "dismissed" }).error).toBeNull();
    }
  });
});

describe("a note beside a dictation", () => {
  test("is shown and leaves it running", () => {
    const listening = after([{ kind: "preparing" }, { kind: "listening" }]);
    expect(dictationStep(listening, { kind: "noted", message: "нет" })).toEqual({ ...listening, error: "нет" });
  });
});

describe("a phrase goes in the box", () => {
  test("at the start, with no space before it", () => {
    expect(insertPhrase("", 0, "Привет.")).toEqual({ text: "Привет.", at: 7 });
    expect(insertPhrase("дальше", 0, "Привет.")).toEqual({ text: "Привет. дальше", at: 7 });
  });

  test("after text, through a space", () => {
    expect(insertPhrase("Да", 2, "Привет.")).toEqual({ text: "Да Привет.", at: 10 });
  });

  test("after a space or at a line's start, as it is", () => {
    expect(insertPhrase("Да ", 3, "Нет.").text).toBe("Да Нет.");
    expect(insertPhrase("Да\n", 3, "Нет.").text).toBe("Да\nНет.");
  });

  test("in the middle, apart from what follows, and the next one after it", () => {
    const first = insertPhrase("ab", 1, "Да.");
    expect(first).toEqual({ text: "a Да. b", at: 5 });
    expect(insertPhrase(first.text, first.at, "Нет.").text).toBe("a Да. Нет. b");
    expect(insertPhrase("a b", 1, "Да.").text).toBe("a Да. b");
  });

  test("right up to the punctuation that follows", () => {
    expect(insertPhrase("Привет, мир", 6, "Раз").text).toBe("Привет Раз, мир");
  });

  test("past a box that got shorter, at its end", () => {
    expect(insertPhrase("ab", 10, "Да.")).toEqual({ text: "ab Да.", at: 6 });
  });
});

describe("the microphone button", () => {
  test("starts when the system allows it, or has not been asked, or does not say", () => {
    for (const access of ["granted", "notAsked", "unknown", undefined] as const) {
      expect(micButton(access, "idle")).toMatchObject({ look: "mic", press: "start" });
    }
    expect(micButton("granted", "idle").title).toContain(shortcutText("dictate"));
  });

  test("refused, opens the system's settings", () => {
    expect(micButton("denied", "idle")).toMatchObject({ look: "refused", press: "settings" });
  });

  test("while getting ready, cancels", () => {
    expect(micButton("granted", "preparing")).toMatchObject({ look: "working", press: "cancel" });
  });

  test("while listening, stops", () => {
    expect(micButton("granted", "listening")).toMatchObject({ look: "stop", press: "stop" });
  });

  test("while the end is recognized, does nothing", () => {
    expect(micButton("granted", "finishing")).toMatchObject({ look: "working", press: null });
  });
});

describe("the key held", () => {
  let started = 0;
  let ended = 0;
  let hold: ReturnType<typeof holdKey>;
  const pass = (ms: number) => new Promise((done) => setTimeout(done, ms));

  beforeEach(() => {
    started = ended = 0;
    hold = holdKey(
      () => started++,
      () => ended++,
    );
  });

  test("starts past the threshold and ends when let go", async () => {
    hold.press();
    await pass(HOLD_MS / 2);
    expect(started).toBe(0);
    await pass(HOLD_MS);
    expect(started).toBe(1);
    expect(ended).toBe(0);
    hold.lift();
    expect(ended).toBe(1);
  });

  test("let go early is nothing", async () => {
    hold.press();
    hold.lift();
    await pass(HOLD_MS * 1.5);
    expect([started, ended]).toEqual([0, 0]);
  });

  test("another key meanwhile is a modifier, not a dictation", async () => {
    hold.press();
    hold.other();
    await pass(HOLD_MS * 1.5);
    hold.lift();
    expect([started, ended]).toEqual([0, 0]);
  });

  test("another key once it runs changes nothing", async () => {
    hold.press();
    await pass(HOLD_MS * 1.5);
    hold.other();
    hold.lift();
    expect([started, ended]).toEqual([1, 1]);
  });

  test("pressed again while held counts once, and it can be held again after", async () => {
    hold.press();
    hold.press();
    await pass(HOLD_MS * 1.5);
    hold.press();
    await pass(HOLD_MS * 1.5);
    expect(started).toBe(1);
    hold.lift();
    hold.lift();
    expect(ended).toBe(1);
    hold.press();
    await pass(HOLD_MS * 1.5);
    expect(started).toBe(2);
    hold.lift();
  });
});

describe("the dictation key", () => {
  const key = (code: string, altKey: boolean) => ({ code, altKey, metaKey: false, ctrlKey: false, shiftKey: false });

  test("is the right modifier and not the left", () => {
    expect(matches(key("AltRight", true), "dictate")).toBe(true);
    expect(matches(key("AltLeft", true), "dictate")).toBe(false);
  });

  test("is known when let go, when it no longer says it is held", () => {
    expect(released(key("AltRight", false), "dictate")).toBe(true);
    expect(released(key("AltLeft", false), "dictate")).toBe(false);
  });

  test("is said in words", () => {
    expect(comboKeys(SHORTCUTS.dictate.combos[0], true)).toEqual(["Right ⌥"]);
    expect(comboKeys(SHORTCUTS.dictate.combos[0], false)).toEqual(["Right Alt"]);
  });
});
