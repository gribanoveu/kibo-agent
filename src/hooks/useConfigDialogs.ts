import { useState } from "react";

/** A config file's dialog: the whole file, one entry of it (null for a new one), or closed. */
export type EntryDialog<K> = "json" | { entry: K | null } | null;

/**
 * The MCP and hooks files open as a whole (JSON) or one entry at a time (a
 * form): `entry` is the server's name or the hook's row, null for a new one.
 * The editing flags keep the file read while its dialog is open, so this is
 * called before `useMcp`/`useHooks` — and takes nothing from them.
 */
export function useConfigDialogs() {
  const [mcpDialog, setMcpDialog] = useState<EntryDialog<string>>(null);
  const [hooksDialog, setHooksDialog] = useState<EntryDialog<number>>(null);
  return {
    mcpDialog,
    hooksDialog,
    mcpEditing: mcpDialog !== null,
    hooksEditing: hooksDialog !== null,
    addMcp: () => setMcpDialog({ entry: null }),
    editMcp: (name: string) => setMcpDialog({ entry: name }),
    editMcpFile: () => setMcpDialog("json"),
    closeMcp: () => setMcpDialog(null),
    addHook: () => setHooksDialog({ entry: null }),
    editHook: (index: number) => setHooksDialog({ entry: index }),
    editHooksFile: () => setHooksDialog("json"),
    closeHooks: () => setHooksDialog(null),
  };
}