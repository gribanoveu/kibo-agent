import { describe, expect, test } from "bun:test";
import { render, screen, fireEvent, within } from "@testing-library/react";
import { Settings } from "../components/Settings";
import type { SkillsView } from "../lib/chat";
import { DEFAULT_ALERTS, type AlertPrefs } from "../lib/attention";

// The dialog's own rules: one section at a time, and the request log — which
// writes whole conversations to disk — stays off until someone turns it on.

const SOURCES: SkillsView["sources"] = [
  { id: "project", path: "/repo", enabled: true },
  { id: "app", path: "/Users/me/.kibo/skills", enabled: true },
  { id: "agents", path: "/Users/me/.agents/skills", enabled: false },
  { id: "claude", path: "/Users/me/.claude/skills", enabled: true },
];

const dialog = (debugLogging = false) => {
  const logging: boolean[] = [];
  const languages: string[] = [];
  const sizes: string[] = [];
  const modes: string[] = [];
  const palettes: string[] = [];
  const wraps: boolean[] = [];
  const alerts: AlertPrefs[] = [];
  const sources: [string, boolean][] = [];
  let logOpened = 0;
  render(
    <Settings
      skills={{
        view: { dir: "/Users/me/.kibo/skills", skills: [], sources: SOURCES },
        error: null,
        onToggle: (id, on) => sources.push([id, on]),
      }}
      provider={{
        settings: { providers: [], activeProviderId: null, debugLogging, replyLanguage: "auto" },
        busy: false,
        error: null,
        onSave: () => {},
        onRemove: () => {},
        onSelect: () => {},
      }}
      debugLogging={debugLogging}
      onDebugLogging={(enabled) => logging.push(enabled)}
      replyLanguage="auto"
      onReplyLanguage={(language) => languages.push(language)}
      theme={{ mode: "system", light: "light", dark: "one-dark" }}
      onThemeMode={(mode) => modes.push(mode)}
      onThemePalette={(theme) => palettes.push(theme)}
      fontSize="large"
      onFontSize={(size) => sizes.push(size)}
      wrapLines
      onWrapLines={(wrap) => wraps.push(wrap)}
      alerts={DEFAULT_ALERTS}
      onAlerts={(next) => alerts.push(next)}
      onOpenLog={() => logOpened++}
      policy={{ provider: null, debugLogging, mcpServers: [], hooks: [] }}
    />,
  );
  return { logging, languages, sizes, wraps, alerts, sources, modes, palettes, logOpened: () => logOpened };
};

describe("the settings dialog", () => {
  test("opens on the model provider, one section at a time", () => {
    dialog();
    expect(screen.getByText("Model provider")).toBeDefined();
    expect(screen.queryByRole("radiogroup", { name: "Theme" })).toBeNull();

    fireEvent.click(screen.getByText("Appearance"));
    expect(screen.getByRole("radiogroup", { name: "Theme" })).toBeDefined();
    expect(screen.queryByText("Model provider")).toBeNull();
  });

  /// Auto leaves /review in English, and the hint says so: that is the case
  /// the setting exists for.
  test("the reply language has its own section, and Auto says what it does to /review", () => {
    const { languages } = dialog();
    fireEvent.click(screen.getByText("Language"));
    const group = screen.getByRole("radiogroup", { name: "Reply language" });
    expect(within(group).getByRole("radio", { name: "Auto" }).getAttribute("aria-checked")).toBe("true");
    expect(screen.getByText(/\/review has nothing you wrote to go by/)).toBeDefined();
    fireEvent.click(within(group).getByRole("radio", { name: "Русский" }));
    expect(languages).toEqual(["russian"]);
  });

  test("a theme is a mode, and a palette for each side, each card drawn in its own", () => {
    const { modes, palettes } = dialog();
    fireEvent.click(screen.getByText("Appearance"));

    // "Dark" is a mode and a palette too: each is asked for in its own group.
    fireEvent.click(within(screen.getByRole("radiogroup", { name: "Theme" })).getByRole("radio", { name: "Dark" }));
    expect(modes).toEqual(["dark"]);

    // One card per palette, drawn in it; the picked one on each side is checked.
    const latte = screen.getByRole("radio", { name: "Latte" });
    expect((latte.querySelector(".theme-preview") as HTMLElement).dataset.theme).toBe("latte");
    expect(screen.getByRole("radio", { name: "One Dark" }).getAttribute("aria-checked")).toBe("true");
    expect(latte.getAttribute("aria-checked")).toBe("false");
    fireEvent.click(latte);
    expect(palettes).toEqual(["latte"]);
  });

  test("Skills lists the folders skills come from, in the order a name is looked up, each with a switch", () => {
    const { sources } = dialog();
    fireEvent.click(screen.getByText("Skills"));
    const titles = ["The repository's", "Kibo's", "Codex and other agents'", "Claude Code's"].map((t) => screen.getByText(t));
    for (let i = 1; i < titles.length; i++) {
      expect(titles[i - 1].compareDocumentPosition(titles[i]) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    }
    expect(screen.getByText("/Users/me/.agents/skills")).toBeDefined();
    expect(screen.getByText("3/4")).toBeDefined();

    fireEvent.click(screen.getAllByRole("button", { pressed: false })[0]);
    fireEvent.click(screen.getAllByRole("button", { pressed: true })[0]);
    expect(sources).toEqual([["agents", true], ["project", false]]);
  });

  test("the font size is picked under Appearance", () => {
    const { sizes } = dialog();
    fireEvent.click(screen.getByText("Appearance"));

    expect(screen.getByRole("radio", { name: "large" }).getAttribute("aria-checked")).toBe("true");
    fireEvent.click(screen.getByRole("radio", { name: "small" }));
    expect(sizes).toEqual(["small"]);
  });

  test("the file viewer's long lines are picked under Appearance", () => {
    const { wraps } = dialog();
    fireEvent.click(screen.getByText("Appearance"));
    const group = screen.getByRole("radiogroup", { name: "Long lines in the file viewer" });
    expect(group.querySelector('[aria-checked="true"]')?.textContent).toBe("wrap");
    fireEvent.click(screen.getByRole("radio", { name: "scroll sideways" }));
    expect(wraps).toEqual([false]);
  });

  test("under Notifications each moment's sound and notification have a switch of their own", () => {
    const { alerts } = dialog();
    fireEvent.click(screen.getByText("Notifications"));
    expect(screen.getAllByRole("switch").every((s) => s.getAttribute("aria-checked") === "true")).toBe(true);
    // A card has no sound to switch: in Ask one comes with every step that writes.
    expect(screen.queryByRole("switch", { name: "Waiting for approval: sound" })).toBeNull();

    fireEvent.click(screen.getByRole("switch", { name: "Finished: sound" }));
    fireEvent.click(screen.getByRole("switch", { name: "Failed: sound" }));
    fireEvent.click(screen.getByRole("switch", { name: "Waiting for approval: notification" }));
    expect(alerts).toEqual([
      { ...DEFAULT_ALERTS, sound: { done: false, failed: true } },
      { ...DEFAULT_ALERTS, sound: { done: true, failed: false } },
      { ...DEFAULT_ALERTS, notify: { done: true, failed: true, approval: false } },
    ]);
  });

  test("the request log is off unless it was turned on", () => {
    const { logging } = dialog();
    fireEvent.click(screen.getByText("Privacy"));

    expect(screen.getByRole("radio", { name: "off" }).getAttribute("aria-checked")).toBe("true");
    fireEvent.click(screen.getByRole("radio", { name: "on" }));
    expect(logging).toEqual([true]);
  });

  test("privacy leads to the tool log", () => {
    const { logOpened } = dialog();
    fireEvent.click(screen.getByText("Privacy"));
    fireEvent.click(screen.getByText("Open the log"));
    expect(logOpened()).toBe(1);
  });
});
