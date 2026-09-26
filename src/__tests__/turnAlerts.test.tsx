import { afterEach, describe, expect, mock, test } from "bun:test";

// The user is called back when the agent is done, failed, or waits on a card:
// a sound when a turn ends (never for a card), and a system notification only
// while the window is in the background — each moment and each way switched on its own.

const played: string[] = [];
mock.module("../lib/sounds", () => ({
  playSound: (sound: string) => {
    played.push(sound);
    return Promise.resolve();
  },
  preloadSounds: () => {},
}));

let focused = false;
document.hasFocus = () => focused;

// What the notification plugin draws with: the webview's `Notification`,
// which Tauri replaces with the system's.
const shown: { title: string; body?: string }[] = [];
class FakeNotification {
  static permission = "granted";
  static requestPermission = () => Promise.resolve("granted");
  constructor(title: string, options?: { body?: string }) {
    shown.push({ title, body: options?.body });
  }
}
(window as unknown as Record<string, unknown>).Notification = FakeNotification;
(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};

const { attentionFor, DEFAULT_ALERTS, isAlertPrefs, soundFor } = await import("../lib/attention");
const { useTurnAlerts } = await import("../hooks/useTurnAlerts");
const { renderHook, waitFor } = await import("@testing-library/react");

afterEach(() => {
  played.length = 0;
  shown.length = 0;
  focused = false;
});

describe("what a change of status asks of the user", () => {
  test("an end the agent reached, or a card, and nothing else", () => {
    expect(attentionFor("running", "done", false)).toBe("done");
    expect(attentionFor("running", "done", true)).toBe("failed");
    expect(attentionFor("running", "awaitingApproval", false)).toBe("approval");
    // Stopped by the user, a chat opened, a turn starting: not news.
    expect(attentionFor("running", "cancelled", false)).toBeNull();
    expect(attentionFor("idle", "done", false)).toBeNull();
    expect(attentionFor("done", "running", false)).toBeNull();
    expect(attentionFor("done", "done", false)).toBeNull();
    // A card still waiting is the same card, not a new one.
    expect(attentionFor("awaitingApproval", "awaitingApproval", false)).toBeNull();
  });
});

type Props = Parameters<typeof useTurnAlerts>[0];

const ALL_ON = DEFAULT_ALERTS;
const base: Props = { status: "running", failed: false, chatTitle: "Fix login", prefs: ALL_ON };

/** A hook that starts at `initial` and is moved through `steps`, each a change to the props before it. */
const run = (steps: Partial<Props>[], initial: Partial<Props> = {}) => {
  let props = { ...base, ...initial };
  const { rerender } = renderHook((p: Props) => useTurnAlerts(p), { initialProps: props });
  for (const step of steps) {
    props = { ...props, ...step };
    rerender(props);
  }
};

const settle = () => new Promise((resolve) => setTimeout(resolve, 20));

describe("calling the user back", () => {
  test("a finished turn plays its sound and, in the background, is a notification named after the chat", async () => {
    run([{ status: "done" }]);
    expect(played).toEqual(["done"]);
    await waitFor(() => expect(shown).toEqual([{ title: "Fix login", body: "The agent has finished." }]));
  });

  test("a failure plays the other sound", async () => {
    run([{ status: "done", failed: true }]);
    expect(played).toEqual(["attention"]);
    await waitFor(() => expect(shown.map((n) => n.body)).toEqual(["The turn failed."]));
  });

  test("a card is not heard, only notified; the end of the turn after it is heard", async () => {
    run([{ status: "awaitingApproval" }, { status: "running" }, { status: "done" }]);
    await waitFor(() =>
      expect(shown.map((n) => n.body)).toEqual(["The agent is waiting for your approval.", "The agent has finished."]),
    );
    expect(played).toEqual(["done"]);
  });

  test("each moment's sound and notification are switched on their own", async () => {
    const prefs = { sound: { done: false, failed: true }, notify: { done: true, failed: false, approval: false } };
    run([{ status: "done" }, { status: "running" }, { status: "done", failed: true }, { status: "awaitingApproval", failed: false }], {
      prefs,
    });
    expect(played).toEqual(["attention"]);
    await waitFor(() => expect(shown.map((n) => n.body)).toEqual(["The agent has finished."]));
    await settle();
    expect(shown).toHaveLength(1);
  });

  test("with the window in front there is a sound but no notification", async () => {
    focused = true;
    run([{ status: "done" }]);
    expect(played).toEqual(["done"]);
    await settle();
    expect(shown).toEqual([]);
  });

  test("a chat without a name yet is notified as the app", async () => {
    run([{ status: "done", chatTitle: null }]);
    await waitFor(() => expect(shown).toEqual([{ title: "Kibo Agent", body: "The agent has finished." }]));
  });

  test("the chat named after its turn has ended, or a card waiting while it is renamed, is still one alert", async () => {
    run(
      [
        { status: "done" },
        { chatTitle: "Fix the login form" },
        { status: "running" },
        { status: "awaitingApproval" },
        { chatTitle: "Login" },
      ],
    );
    await waitFor(() => expect(shown).toHaveLength(2));
    await settle();
    expect(shown).toHaveLength(2);
    expect(played).toEqual(["done"]);
  });

  test("changing the settings after the end is not a second alert", async () => {
    run([{ prefs: { ...ALL_ON } }, { chatTitle: "Renamed" }], { status: "done" });
    await settle();
    expect(played).toEqual([]);
    expect(shown).toEqual([]);
  });
});

describe("the stored preferences", () => {
  test("a value of another shape falls back to the defaults", () => {
    expect(isAlertPrefs(DEFAULT_ALERTS)).toBe(true);
    expect(isAlertPrefs(true)).toBe(false);
    expect(isAlertPrefs({ sound: { done: true, failed: true }, notify: { done: true, failed: true } })).toBe(false);
    expect(isAlertPrefs({ sound: { done: "yes", failed: true }, notify: DEFAULT_ALERTS.notify })).toBe(false);
  });

  test("a card has no sound, even with every sound on", () => {
    expect(soundFor("approval", DEFAULT_ALERTS)).toBeNull();
    expect(soundFor("done", DEFAULT_ALERTS)).toBe("done");
    expect(soundFor("failed", DEFAULT_ALERTS)).toBe("attention");
  });
});

describe("with messages queued", () => {
  test("a turn that finished with the next one waiting is not the end: no alert", async () => {
    run([{ status: "done", queued: 1 }]);
    await settle();
    expect(played).toEqual([]);
    expect(shown).toEqual([]);
  });

  test("a failure is still told", async () => {
    run([{ status: "done", failed: true, queued: 1 }]);
    expect(played).toEqual(["attention"]);
  });
});
