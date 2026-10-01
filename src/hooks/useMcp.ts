import { useCallback, useEffect, useState } from "react";
import { connectMcpServer, mcpConfig, saveMcpConfig, setMcpServerEnabled, setMcpToolShown, type McpView } from "../lib/chat";

/**
 * The MCP configuration, re-read whenever the tab or the editor opens: the
 * file is the user's, and may have been edited outside the app. Re-read too
 * when `refreshKey` changes while shown — the turn's status, since a turn is
 * what starts the servers and what finds one gone.
 */
export function useMcp(visible: boolean, refreshKey?: unknown) {
  const [view, setView] = useState<McpView | null>(null);
  const [error, setError] = useState<string | null>(null);

  const reload = useCallback(async () => {
    try {
      setView(await mcpConfig());
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    if (visible) reload();
  }, [visible, refreshKey, reload]);

  /** Resolves to whether it was stored; a refusal is left in `error` for the editor to show. */
  const save = useCallback(async (text: string) => {
    try {
      setView(await saveMcpConfig(text));
      setError(null);
      return true;
    } catch (e) {
      setError(String(e));
      return false;
    }
  }, []);

  const setEnabled = useCallback(async (name: string, enabled: boolean) => {
    setView((v) => v && { ...v, servers: v.servers.map((s) => (s.name === name ? { ...s, enabled } : s)) });
    try {
      setView(await setMcpServerEnabled(name, enabled));
      setError(null);
    } catch (e) {
      setError(String(e));
      await reload();
    }
  }, [reload]);

  /** One tool's switch. Not drawn ahead of the answer: whether it ends up direct or deferred is the backend's to say. */
  const setToolShown = useCallback(async (server: string, tool: string, shown: boolean) => {
    try {
      setView(await setMcpToolShown(server, tool, shown));
      setError(null);
    } catch (e) {
      setError(String(e));
      await reload();
    }
  }, [reload]);

  /**
   * Starts one server to list its tools, for a row the user just opened.
   * The row says "starting" meanwhile: a first `npx` run takes seconds.
   */
  const connect = useCallback(async (name: string) => {
    setView((v) => v && { ...v, servers: v.servers.map((s) => (s.name === name && s.state.state === "notStarted" ? { ...s, state: { state: "starting" } } : s)) });
    try {
      setView(await connectMcpServer(name));
      setError(null);
    } catch (e) {
      setError(String(e));
      await reload();
    }
  }, [reload]);

  return { view, error, save, setEnabled, setToolShown, connect };
}
