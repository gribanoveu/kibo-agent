import { describe, expect, test } from "bun:test";
import { render, screen, fireEvent, within } from "@testing-library/react";
import { Settings } from "../components/Settings";
import type { SkillsView } from "../lib/skills";
import { DEFAULT_ALERTS, type AlertPrefs } from "../lib/attention";
import type { MadeSceneBody, SceneChoice } from "../hooks/useScene";
import type { ThemeChoice } from "../hooks/useTheme";

// The dialog's own rules: one section at a time, and the request log — which
// writes whole conversations to disk — stays off until someone turns it on.

const SOURCES: SkillsView["sources"] = [
  { id: "project", path: "/repo", enabled: true },
  { id: "app", path: "/Users/me/.kibo/skills", enabled: true },
  { id: "agents", path: "/Users/me/.agents/skills", enabled: false },
  { id: "claude", path: "/Users/me/.claude/skills", enabled: true },
];

const dialog = (
  debugLogging = false,
  skills: SkillsView["skills"] = [],
  theme: ThemeChoice = { mode: "system", light: "linen", dark: "basalt" },
  active: SceneChoice = null,
  made: MadeSceneBody[] = [],
  sceneError: string | null = null,
) => {
  const logging: boolean[] = [];
  const languages: string[] = [];
  const limits: { rounds: number; budget: number; parallel: number }[] = [];
  const sizes: string[] = [];
  const modes: string[] = [];
  const palettes: string[] = [];
  const wraps: boolean[] = [];
  const alerts: AlertPrefs[] = [];
  const sources: [string, boolean][] = [];
  const scenes: SceneChoice[] = [];
  const motion: boolean[] = [];
  const deleted: string[] = [];
  let logOpened = 0;
  render(
    <Settings
      skills={{
        view: { dir: "/Users/me/.kibo/skills", skills, sources: SOURCES },
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
      turnLimits={{ rounds: 60, budget: 250, parallel: 3 }}
      onTurnLimits={(next) => limits.push(next)}
      theme={theme}
      onThemeMode={(mode) => modes.push(mode)}
      onThemePalette={(theme) => palettes.push(theme)}
      scene={{
        active,
        onChoose: (scene) => scenes.push(scene),
        made,
        onDelete: (id) => deleted.push(id),
        error: sceneError,
        animateInBackground: false,
        onAnimateInBackground: (on) => motion.push(on),
      }}
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
  return { limits, logging, languages, sizes, wraps, alerts, sources, modes, palettes, scenes, motion, deleted, logOpened: () => logOpened };
};

const cardName = (card: HTMLElement) => card.querySelector(".theme-card-name")?.textContent;

describe("the settings dialog", () => {
  test("a turn limit is saved as its field is left, and a zero goes back to the saved value", () => {
    const { limits } = dialog();
    fireEvent.click(screen.getByText("Agent"));
    const rounds = screen.getByLabelText("Rounds per turn") as HTMLInputElement;

    fireEvent.change(rounds, { target: { value: "0" } });
    fireEvent.blur(rounds);
    expect(limits).toEqual([]);
    expect(rounds.value).toBe("60");

    fireEvent.change(rounds, { target: { value: "200" } });
    fireEvent.blur(rounds);
    expect(limits).toEqual([{ rounds: 200, budget: 250, parallel: 3 }]);
  });

  test("the parallel-turns limit is its own field and is saved the same way", () => {
    const { limits } = dialog();
    fireEvent.click(screen.getByText("Agent"));
    const parallel = screen.getByLabelText("Parallel turns") as HTMLInputElement;

    fireEvent.change(parallel, { target: { value: "5" } });
    fireEvent.blur(parallel);
    expect(limits).toEqual([{ rounds: 60, budget: 250, parallel: 5 }]);
  });

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
    const chalk = screen.getByRole("radio", { name: "Chalk" });
    expect((chalk.querySelector(".theme-preview") as HTMLElement).dataset.theme).toBe("chalk");
    expect(screen.getByRole("radio", { name: "Basalt" }).getAttribute("aria-checked")).toBe("true");
    expect(chalk.getAttribute("aria-checked")).toBe("false");
    fireEvent.click(chalk);
    expect(palettes).toEqual(["chalk"]);
  });

  test("Frost and its scenes are a group of their own, beside the light and dark palettes", () => {
    const { palettes, scenes, motion } = dialog();
    fireEvent.click(screen.getByText("Appearance"));
    expect(within(screen.getByRole("radiogroup", { name: "Dark theme" })).queryByRole("radio", { name: "Frost" })).toBeNull();

    const frost = screen.getByRole("radiogroup", { name: "Frost" });
    const cards = within(frost).getAllByRole("radio");
    expect(cards.map(cardName)).toEqual(["Frost", "Frost Ferns", "Glacier Mist", "Snowfall"]);
    // Dark is Basalt: nothing in the group is what shows.
    expect(cards.filter((c) => c.getAttribute("aria-checked") === "true").map(cardName)).toEqual([]);
    // A scene's card is its still picture, in a frame with no scripts.
    const picture = cards[3].querySelector("iframe")!;
    expect(picture.getAttribute("sandbox")).toBe("");
    expect(picture.getAttribute("srcdoc")).toContain('<html class="paused">');
    expect(cards[0].querySelector("iframe")).toBeNull();

    fireEvent.click(cards[3]);
    expect(palettes).toEqual(["frost"]);
    expect(scenes).toEqual([{ kind: "builtin", id: "snowfall" }]);
    fireEvent.click(cards[0]);
    expect(scenes.at(-1)).toBeNull();

    const background = screen.getByRole("radiogroup", { name: "Scene while another app is in front" });
    expect(within(background).getByRole("radio", { name: "paused" }).getAttribute("aria-checked")).toBe("true");
    fireEvent.click(within(background).getByRole("radio", { name: "moving" }));
    expect(motion).toEqual([true]);
  });

  test("under Frost, the card checked is the scene that shows, and the badge is the Frost group's", () => {
    dialog(false, [], { mode: "dark", light: "linen", dark: "frost" }, { kind: "builtin", id: "glacier-mist" });
    fireEvent.click(screen.getByText("Appearance"));
    const checked = within(screen.getByRole("radiogroup", { name: "Frost" }))
      .getAllByRole("radio")
      .filter((c) => c.getAttribute("aria-checked") === "true");
    expect(checked.map(cardName)).toEqual(["Glacier Mist"]);
    const badges = screen.getAllByText("showing").map((b) => b.parentElement?.textContent);
    expect(badges).toEqual(["Frostshowing"]);
  });

  test("the scenes the model painted are a group of their own, there only once there is one", () => {
    dialog();
    fireEvent.click(screen.getByText("Appearance"));
    expect(screen.queryByRole("radiogroup", { name: "Generated" })).toBeNull();
  });

  test("a painted scene is chosen like a built-in one, and deleted only after a yes", () => {
    const made = [
      { id: "b", title: "Lake", created: 2, body: "<i class=lake></i>" },
      { id: "a", title: "Forest", created: 1, body: "<i class=forest></i>" },
    ];
    const { palettes, scenes, deleted } = dialog(false, [], { mode: "dark", light: "linen", dark: "frost" }, { kind: "made", id: "a" }, made);
    fireEvent.click(screen.getByText("Appearance"));
    const group = screen.getByRole("radiogroup", { name: "Generated" });
    const cards = within(group).getAllByRole("radio");
    expect(cards.map(cardName)).toEqual(["Lake", "Forest"]);
    expect(cards.filter((c) => c.getAttribute("aria-checked") === "true").map(cardName)).toEqual(["Forest"]);
    expect(cards[1].querySelector("iframe")?.getAttribute("srcdoc")).toContain("<i class=forest></i>");
    // The badge is the group whose card shows, and Frost bare is not checked.
    expect(screen.getAllByText("showing").map((b) => b.parentElement?.textContent)).toEqual(["Generatedshowing"]);
    expect(within(screen.getByRole("radiogroup", { name: "Frost" })).getByRole("radio", { name: "Frost" }).getAttribute("aria-checked")).toBe("false");

    fireEvent.click(cards[0]);
    expect(palettes).toEqual(["frost"]);
    expect(scenes).toEqual([{ kind: "made", id: "b" }]);

    fireEvent.click(within(group).getByRole("button", { name: "Delete Lake" }));
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(deleted).toEqual([]);
    expect(screen.queryByRole("group", { name: "Delete Lake" })).toBeNull();
    fireEvent.click(within(group).getByRole("button", { name: "Delete Lake" }));
    fireEvent.click(within(screen.getByRole("group", { name: "Delete Lake" })).getByRole("button", { name: "Delete" }));
    expect(deleted).toEqual(["b"]);
  });

  test("a scene that could not be deleted says why", () => {
    dialog(false, [], undefined, null, [], "the scenes' store failed: disk full");
    fireEvent.click(screen.getByText("Appearance"));
    expect(screen.getByText("the scenes' store failed: disk full")).toBeDefined();
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

  test("a folder opens to the skills it gave; one switched off says it is not read", () => {
    const skill = { enabled: true, error: null, shadowedBy: null };
    dialog(false, [
      { ...skill, name: "deploy", description: "Ships it", source: "project", path: "/repo/.claude/skills/deploy" },
      { ...skill, name: "pdf", description: "Reads PDFs", source: "user", path: "/Users/me/.claude/skills/pdf" },
    ]);
    fireEvent.click(screen.getByText("Skills"));
    expect(screen.queryByText("pdf")).toBeNull();

    fireEvent.click(screen.getByText("Claude Code's"));
    expect(screen.getByText("pdf")).toBeDefined();
    expect(screen.getByText("Reads PDFs")).toBeDefined();
    expect(screen.queryByText("deploy")).toBeNull();

    fireEvent.click(screen.getByText("Kibo's"));
    expect(screen.getByText("No skills here.")).toBeDefined();
    fireEvent.click(screen.getByText("Codex and other agents'"));
    expect(screen.getByText("Switched off — not read.")).toBeDefined();
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
