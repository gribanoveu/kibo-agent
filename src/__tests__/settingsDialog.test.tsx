import { describe, expect, test, mock } from "bun:test";
import { fireEvent, render, screen } from "@testing-library/react";
import { DEFAULT_ALERTS, type AlertPrefs } from "../lib/attention";
import { SettingsDialog } from "../components/SettingsDialog";

const played: string[] = [];
mock.module("../lib/sounds", () => ({
  playSound: async (sound: string) => {
    played.push(sound);
  },
  preloadSounds: () => {},
  playTone: async () => {},
}));

/** The dialog with the least Settings needs, as Settings.test.tsx drives it. */
const setup = (alerts: AlertPrefs, open = true) => {
  const picked: AlertPrefs[] = [];
  let logOpened = 0;
  const view = render(
    <SettingsDialog
      open={open}
      onClose={() => {}}
      skills={{ view: { dir: "/u/skills", skills: [], sources: [] }, error: null, onToggle: () => {} }}
      provider={{
        settings: { providers: [], activeProviderId: null, debugLogging: false, replyLanguage: "auto" },
        busy: false,
        error: null,
        onSave: () => {},
        onRemove: () => {},
        onSelect: () => {},
      }}
      debugLogging={false}
      onDebugLogging={() => {}}
      replyLanguage="auto"
      onReplyLanguage={() => {}}
      turnLimits={{ rounds: 60, budget: 250, parallel: 3 }}
      onTurnLimits={() => {}}
      theme={{ mode: "system", light: "linen", dark: "basalt" }}
      onThemeMode={() => {}}
      onThemePalette={() => {}}
      fontSize="large"
      onFontSize={() => {}}
      wrapLines
      onWrapLines={() => {}}
      alerts={alerts}
      onAlerts={(next) => picked.push(next)}
      onOpenLog={() => logOpened++}
      policy={{ provider: null, debugLogging: false, mcpServers: [], hooks: [] }}
    />,
  );
  return { ...view, picked, logOpened: () => logOpened };
};

describe("the settings dialog", () => {
  test("closed draws nothing; open draws the settings", () => {
    const { container } = setup(DEFAULT_ALERTS, false);
    expect(container.firstChild).toBeNull();
    setup(DEFAULT_ALERTS);
    expect(screen.getByText("Model provider")).toBeTruthy();
  });

  test("a sound turned on is heard once, in its own voice", () => {
    played.length = 0;
    const { picked } = setup({ ...DEFAULT_ALERTS, sound: { done: false, failed: false } });
    fireEvent.click(screen.getByText("Notifications"));
    fireEvent.click(screen.getByRole("switch", { name: "Finished: sound" }));
    expect(played).toEqual(["done"]);
    expect(picked).toEqual([{ ...DEFAULT_ALERTS, sound: { done: true, failed: false } }]);
    fireEvent.click(screen.getByRole("switch", { name: "Failed: sound" }));
    expect(played).toEqual(["done", "attention"]);
  });

  test("turning a sound off is a pick like any other, but silent", () => {
    played.length = 0;
    const { picked } = setup(DEFAULT_ALERTS);
    fireEvent.click(screen.getByText("Notifications"));
    fireEvent.click(screen.getByRole("switch", { name: "Finished: sound" }));
    expect(played).toEqual([]);
    expect(picked).toEqual([{ ...DEFAULT_ALERTS, sound: { done: false, failed: true } }]);
  });

  test("the log opens from Privacy through the caller's own handler", () => {
    const { logOpened } = setup(DEFAULT_ALERTS);
    fireEvent.click(screen.getByText("Privacy"));
    fireEvent.click(screen.getByText("Open the log"));
    expect(logOpened()).toBe(1);
  });
});