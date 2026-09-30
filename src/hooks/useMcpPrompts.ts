import { useCallback, useEffect, useState } from "react";
import { mcpPrompts, type McpPromptItem } from "../lib/chat";

/**
 * The prompts of the MCP servers running for the open folder, for the `/`
 * menu. Read when another folder is opened, when a turn starts or ends — the
 * turn is what starts servers and re-reads their lists — and each time the
 * menu opens. Nothing is started to be asked, and a list that cannot be read
 * is no prompts.
 */
export function useMcpPrompts(workspace: string | null, turnStatus: unknown) {
  const [prompts, setPrompts] = useState<McpPromptItem[]>([]);

  const reload = useCallback(() => {
    mcpPrompts().then(setPrompts, () => setPrompts([]));
  }, []);

  useEffect(reload, [workspace, turnStatus, reload]);

  return { prompts, reload };
}
