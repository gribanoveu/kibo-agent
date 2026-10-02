import { useCallback, useEffect, useRef, useState } from "react";
import { mcpPrompts, startMcpPrompts, type McpPromptItem } from "../lib/chat";

/** How long a start may take before the menu says it is waiting: servers already running answer at once, and the line would flash. */
const SAY_STARTING_AFTER_MS = 150;

/**
 * The prompts of the MCP servers for the open folder, for the `/` menu. Read
 * when another folder is opened and when a turn starts or ends — the turn is
 * what starts servers and re-reads their lists. `start`, called as the menu
 * opens, starts the servers not running yet first: typing `/` is asking what
 * there is. A list that cannot be read is no prompts.
 */
export function useMcpPrompts(workspace: string | null, turnStatus: unknown) {
  const [prompts, setPrompts] = useState<McpPromptItem[]>([]);
  const [starting, setStarting] = useState(false);
  const pending = useRef(false);

  const reload = useCallback(() => {
    mcpPrompts().then(setPrompts, () => setPrompts([]));
  }, []);

  useEffect(reload, [workspace, turnStatus, reload]);

  const start = useCallback(() => {
    if (pending.current) return;
    pending.current = true;
    const say = setTimeout(() => setStarting(true), SAY_STARTING_AFTER_MS);
    startMcpPrompts()
      .then(setPrompts, () => setPrompts([]))
      .finally(() => {
        clearTimeout(say);
        pending.current = false;
        setStarting(false);
      });
  }, []);

  return { prompts, reload, start, starting };
}
