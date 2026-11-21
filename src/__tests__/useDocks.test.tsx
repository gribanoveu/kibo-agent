import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { act, fireEvent, renderHook } from "@testing-library/react";
import { useDocks } from "../hooks/useDocks";
import { isBoolean, useStoredState } from "../hooks/useStoredState";
import { PANES } from "../components/panes";

// The window's width, which the hook's two media queries answer against.
let width = 3000;
const originalMatchMedia = window.matchMedia;
beforeEach(() => {
  width = 3000;
  window.matchMedia = ((query: string) => ({
    get matches() {
      const min = /min-width: (\d+)px/.exec(query);
      return width >= Number(min?.[1] ?? 0);
    },
    addEventListener: () => {},
    removeEventListener: () => {},
  })) as unknown as typeof window.matchMedia;
});
afterEach(() => {
  window.matchMedia = originalMatchMedia;
  localStorage.clear();
});

type Over = Partial<{
  agentMode: boolean;
  page: { dock: boolean } | null;
  viewerOpen: boolean;
  terminalPaste: { command: string } | null;
}>;

/** The hook as App drives it: the IDE flag is real stored state, the viewer's calls are counted. */
function setup(over: Over = {}) {
  const calls = { close: 0, swap: 0 };
  const view = renderHook(
    (o: Over) => {
      const [ideLayout, setIdeLayout] = useStoredState("test-ide-layout", false, isBoolean);
      return {
        ideLayout,
        layout: useDocks({
          agentMode: o.agentMode ?? true,
          ideLayout,
          setIdeLayout,
          page: o.page ?? null,
          panes: PANES,
          viewerOpen: o.viewerOpen ?? false,
          closeViewer: () => {
            calls.close++;
          },
          swapViewer: () => {
            calls.swap++;
          },
          terminalPaste: o.terminalPaste ?? null,
        }),
      };
    },
    { initialProps: over },
  );
  return { ...view, calls };
}

describe("the window's layout", () => {
  test("openTab puts a pane in its dock; the column stays beside the terminal", () => {
    const { result } = setup();
    act(() => result.current.layout.openTab("changes"));
    expect(result.current.layout.tab).toBe("changes");
    expect(result.current.layout.topHidden).toBe(false);
    expect(result.current.layout.shown("changes")).toBe(true);
    act(() => result.current.layout.openTab("terminal"));
    expect(result.current.layout.bottomShown).toBe("terminal");
    expect(result.current.layout.shown("terminal")).toBe(true);
    expect(result.current.layout.shown("changes")).toBe(true);
    act(() => result.current.layout.closeBottom());
    expect(result.current.layout.bottomShown).toBeNull();
    act(() => result.current.layout.hideAside());
    expect(result.current.layout.topHidden).toBe(true);
    expect(result.current.layout.shown("changes")).toBe(false);
  });

  test("the terminal's key opens and closes it, taking the typing from the composer", () => {
    const { result } = setup();
    const input = document.createElement("input");
    document.body.appendChild(input);
    input.focus();
    act(() => {
      fireEvent.keyDown(window, { code: "Backquote", ctrlKey: true });
    });
    expect(result.current.layout.bottomShown).toBe("terminal");
    expect(document.activeElement).not.toBe(input);
    act(() => {
      fireEvent.keyDown(window, { code: "Backquote", ctrlKey: true });
    });
    expect(result.current.layout.bottomShown).toBeNull();
    input.remove();
  });

  test("the IDE swap opens the tree and puts back the panes it found", () => {
    const { result, calls } = setup();
    act(() => result.current.layout.openTab("changes"));
    act(() => result.current.layout.openTab("terminal"));
    act(() => result.current.layout.toggleIde());
    expect(result.current.ideLayout).toBe(true);
    expect(result.current.layout.tab).toBe("files");
    expect(result.current.layout.bottomShown).toBe("terminal");
    expect(calls.swap).toBe(1);
    act(() => result.current.layout.toggleIde());
    expect(result.current.ideLayout).toBe(false);
    expect(result.current.layout.tab).toBe("changes");
    expect(result.current.layout.bottomShown).toBe("terminal");
    expect(calls.swap).toBe(2);
  });

  test("the swap puts back the panes as asked, not as a narrow window showed them", () => {
    const { result, rerender } = setup();
    act(() => result.current.layout.openTab("changes"));
    width = 300;
    rerender({});
    expect(result.current.layout.topHidden).toBe(true);
    act(() => result.current.layout.toggleIde());
    width = 3000;
    rerender({});
    act(() => result.current.layout.toggleIde());
    expect(result.current.layout.tab).toBe("changes");
    expect(result.current.layout.topHidden).toBe(false);
  });

  test("a pane asked for with no room closes the viewer — the plan's sheet does not", () => {
    const { result, calls, rerender } = setup({ viewerOpen: true });
    width = 300;
    rerender({ viewerOpen: true });
    act(() => result.current.layout.setDocks({ top: "changes", topHidden: false, bottom: null }));
    expect(calls.close).toBe(1);
    act(() => result.current.layout.setDocks({ top: "plan", topHidden: false, bottom: null }));
    expect(calls.close).toBe(1);
    width = 3000;
    rerender({ viewerOpen: true });
    act(() => result.current.layout.setDocks({ top: "changes", topHidden: false, bottom: null }));
    expect(calls.close).toBe(1);
  });

  test("a narrow window falls back to the rail and hides the column; wider again, both return", () => {
    const { result, rerender } = setup();
    act(() => result.current.layout.openTab("changes"));
    width = 300;
    rerender({});
    expect(result.current.layout.rail).toBe(true);
    expect(result.current.layout.dockShown).toBe(false);
    width = 3000;
    rerender({});
    expect(result.current.layout.rail).toBe(false);
    expect(result.current.layout.dockShown).toBe(true);
  });

  test("the sidebar held on its rail by the viewer closes it; collapsed by choice, it toggles back", () => {
    const { result, calls, rerender } = setup({ viewerOpen: true });
    width = 300;
    rerender({ viewerOpen: true });
    act(() => result.current.layout.toggleSidebar());
    expect(calls.close).toBe(1);
    width = 3000;
    rerender({ viewerOpen: true });
    act(() => result.current.layout.setCollapsed(true));
    expect(result.current.layout.rail).toBe(true);
    act(() => result.current.layout.toggleSidebar());
    expect(result.current.layout.rail).toBe(false);
    expect(calls.close).toBe(1);
  });

  test("the plan reads in a sheet over the chat, its key toggling the wish even with no room", () => {
    const { result, rerender } = setup();
    act(() => result.current.layout.openTab("plan"));
    expect(result.current.layout.planSheet).toBe(true);
    expect(result.current.layout.dockShown).toBe(false);
    width = 300;
    rerender({});
    act(() => result.current.layout.openTab("plan"));
    expect(result.current.layout.planSheet).toBe(true);
    expect(result.current.layout.docks.topHidden).toBe(false);
  });

  test("Chat mode shows no terminal under the box and no plan sheet; a page without a dock hides the column", () => {
    const chat = setup({ agentMode: false });
    act(() => chat.result.current.layout.openTab("terminal"));
    expect(chat.result.current.layout.bottomShown).toBeNull();
    act(() => chat.result.current.layout.openTab("plan"));
    expect(chat.result.current.layout.planSheet).toBe(false);
    const dockedPage = setup({ agentMode: false, page: { dock: true } });
    act(() => dockedPage.result.current.layout.openTab("mcp"));
    expect(dockedPage.result.current.layout.topHidden).toBe(false);
    const fullPage = setup({ agentMode: false, page: { dock: false } });
    act(() => fullPage.result.current.layout.openTab("mcp"));
    expect(fullPage.result.current.layout.topHidden).toBe(true);
    expect(fullPage.result.current.layout.planSheet).toBe(false);
    const agentPage = setup({ page: { dock: false } });
    act(() => agentPage.result.current.layout.openTab("plan"));
    expect(agentPage.result.current.layout.planSheet).toBe(false);
  });

  test("a command asked for the shell opens the terminal; none asked leaves it closed", () => {
    const { result, rerender } = setup();
    expect(result.current.layout.bottomShown).toBeNull();
    rerender({ terminalPaste: { command: "ls" } });
    expect(result.current.layout.bottomShown).toBe("terminal");
  });
});