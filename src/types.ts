// The transcript's own types live in `lib/chatTurnReducer.ts`, beside the code
// that builds them, and a saved chat's in `lib/chat.ts`, beside the commands
// that carry it. What stays here is what the panels around the chat are still
// drawn from.

export const ASIDE_TABS = ["overview", "changes", "plan", "mcp", "hooks", "skills", "rules", "files", "terminal"] as const;
export type AsideTab = (typeof ASIDE_TABS)[number];
export const isAsideTab = (value: unknown): value is AsideTab => ASIDE_TABS.includes(value as AsideTab);

/** The sidebar's modes, in its order: the agent in the open folder, or a plain chat with the model. */
export const APP_MODES = ["agent", "chat"] as const;
export type AppMode = (typeof APP_MODES)[number];
export const isAppMode = (value: unknown): value is AppMode => APP_MODES.includes(value as AppMode);

export type PanelItem = {
  id: string;
  badge: string;
  kind: "mcp" | "hook" | "skill" | "rule";
  title: string;
  status?: { label: string; tone: "ok" | "warn" | "off" };
  desc: string;
  meta?: string;
  tags?: string[];
  enabled?: boolean;
  /** Drawn grey: there, but not in use. */
  dim?: boolean;
  /** `enabled` gives a row its own switch, and an off row is drawn grey. `id` is what the switch reports; `name` when absent. */
  rows?: { id?: string; name: string; desc: string; enabled?: boolean }[];
  note?: string;
  source?: string;
};
