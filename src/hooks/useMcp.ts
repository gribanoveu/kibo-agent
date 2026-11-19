import { useCallback, useEffect, useRef, useState } from "react";
import { connectMcpServer, mcpConfig, type McpView, onMcpChanged, saveMcpConfig, setMcpServerEnabled, setMcpToolShown } from "../lib/mcp";

/**
 * The MCP configuration, with each server's state in `workspace` — every open
 * folder runs servers of its own. Re-read whenever the tab or the editor opens, or the folder changes: the
 * file is the user's, and may have been edited outside the app. Re-read too,
 * while shown, each time the backend says a server started, ended or was
 * stopped, and when `refreshKey` changes — the turn's status.
 */
export function useMcp(visible: boolean, workspace: string | null, refreshKey?: unknown) {
  const [view, setView] = useState<McpView | null>(null);
  const [error, setError] = useState<string | null>(null);
  // A start can take seconds; an answer for a folder left meanwhile is not this one's.
  const shownFolder = useRef(workspace);
  shownFolder.current = workspace;

  const reload = useCallback(async () => {
    try {
      const read = await mcpConfig(workspace);
      if (shownFolder.current !== workspace) return;
      setView(read);
      setError(null);
    } catch (e) {
      if (shownFolder.current === workspace) setError(String(e));
    }
  }, [workspace]);

  useEffect(() => {
    if (visible) reload();
  }, [visible, refreshKey, reload]);

  useEffect(() => {
    if (!visible) return;
    let live = true;
    let unlisten: (() => void) | undefined;
    onMcpChanged(() => live && void reload()).then((off) => (live ? (unlisten = off) : off()));
    return () => {
      live = false;
      unlisten?.();
    };
  }, [visible, reload]);

  /** Resolves to whether it was stored; a refusal is left in `error` for the editor to show. */
  const save = useCallback(async (text: string) => {
    try {
      const saved = await saveMcpConfig(text, workspace);
      if (shownFolder.current === workspace) {
        setView(saved);
        setError(null);
      }
      return true;
    } catch (e) {
      if (shownFolder.current === workspace) setError(String(e));
      return false;
    }
  }, [workspace]);

  const setEnabled = useCallback(async (name: string, enabled: boolean) => {
    setView((v) => v && { ...v, servers: v.servers.map((s) => (s.name === name ? { ...s, enabled } : s)) });
    try {
      const next = await setMcpServerEnabled(name, enabled, workspace);
      if (shownFolder.current !== workspace) return;
      setView(next);
      setError(null);
    } catch (e) {
      if (shownFolder.current === workspace) setError(String(e));
      await reload();
    }
  }, [reload, workspace]);

  /** One tool's switch. Not drawn ahead of the answer: whether it ends up direct or deferred is the backend's to say. */
  const setToolShown = useCallback(async (server: string, tool: string, shown: boolean) => {
    try {
      const next = await setMcpToolShown(server, tool, shown, workspace);
      if (shownFolder.current !== workspace) return;
      setView(next);
      setError(null);
    } catch (e) {
      if (shownFolder.current === workspace) setError(String(e));
      await reload();
    }
  }, [reload, workspace]);

  /**
   * Starts one server to list its tools, for a row the user just opened.
   * The row says "starting" meanwhile: a first `npx` run takes seconds.
   */
  const connect = useCallback(async (name: string) => {
    setView((v) => v && { ...v, servers: v.servers.map((s) => (s.name === name && s.state.state === "notStarted" ? { ...s, state: { state: "starting" } } : s)) });
    try {
      const answer = await connectMcpServer(name, workspace);
      if (shownFolder.current !== workspace) return;
      setView(answer);
      setError(null);
    } catch (e) {
      if (shownFolder.current === workspace) setError(String(e));
      await reload();
    }
  }, [reload, workspace]);

  return { view, error, save, setEnabled, setToolShown, connect };
}
