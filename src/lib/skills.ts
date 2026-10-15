import { invoke } from "@tauri-apps/api/core";
import { inTauri, requireBackend } from "./backend";

// Skills and the open folder's rules, as the Rules and Skills tabs show them.

/**
 * Mirrors `domain::skills::SkillListItem`: one skill folder, the open
 * folder's (`.claude/skills`, `.agents/skills`) or the user's (the app's,
 * `~/.agents/skills`, `~/.claude/skills`). `error` is set when its SKILL.md did
 * not parse. The switch is by name — two rows can share one — and `path` tells
 * rows apart. `shadowedBy` is the folder of the skill used instead of this one.
 */
export type SkillListItem = {
  name: string;
  description: string;
  enabled: boolean;
  error: string | null;
  source: "project" | "user";
  path: string;
  shadowedBy: string | null;
};

/**
 * Mirrors `domain::skills::SkillSourceItem`: a folder skills are read from —
 * `project` (the repository's, `path` its root, empty with no folder open),
 * `app`, `agents`, `claude` — and whether it is read at all.
 */
export type SkillSourceItem = { id: "project" | "app" | "agents" | "claude"; path: string; enabled: boolean };

/** `dir` is where the user's own skills go, so an empty list can say so. */
export type SkillsView = { dir: string; skills: SkillListItem[]; sources: SkillSourceItem[] };

export async function skillsList(): Promise<SkillsView> {
  if (!inTauri()) return { dir: "", skills: [], sources: [] };
  return invoke<SkillsView>("skills_list");
}

/** A command the user wrote in `.kibo/commands`. Mirrors `domain::slash_commands::CommandFile`. */
export type CommandFile = {
  name: string;
  description: string;
  argumentHint: string | null;
  /** Names for the arguments by position, from `arguments` in the frontmatter. */
  arguments: string[];
  /** The prompt; `expandTemplate` puts what is typed after the name into it. */
  template: string;
  source: "project" | "user";
};

/** The open folder's command files and the user's, the folder's first on a shared name. */
export async function commandFilesList(): Promise<CommandFile[]> {
  if (!inTauri()) return [];
  return invoke<CommandFile[]>("slash_commands_list");
}

/**
 * Whether the open folder is trusted with its own skills and `/` commands:
 * `folder` is the one answered (the repository's root), `needed` whether it has
 * any, `trusted` the answer — `null` when never asked.
 */
export type FolderTrust = { folder: string; needed: boolean; trusted: boolean | null };

/** `null` while no folder is open. */
export async function folderTrust(): Promise<FolderTrust | null> {
  if (!inTauri()) return null;
  return invoke<FolderTrust | null>("folder_trust_get");
}

export async function setFolderTrust(trusted: boolean): Promise<void> {
  requireBackend();
  return invoke<void>("folder_trust_set", { trusted });
}

/** A skills folder read or not, for every repository. */
export async function setSkillSourceEnabled(id: SkillSourceItem["id"], enabled: boolean): Promise<void> {
  requireBackend();
  return invoke<void>("skills_set_source_enabled", { id, enabled });
}

/** By name: off in every folder and every repository. */
export async function setSkillEnabled(name: string, enabled: boolean): Promise<void> {
  requireBackend();
  return invoke<void>("skills_set_enabled", { name, enabled });
}

// ---------------------------------------------------------------- project rules

/** An instruction file at the open folder's root. `error` when it cannot be sent; then it has no switch. */
export type RuleListItem = {
  name: string;
  path: string;
  enabled: boolean;
  content: string;
  truncated: boolean;
  error: string | null;
};

export async function rulesList(): Promise<RuleListItem[]> {
  if (!inTauri()) return [];
  return invoke<RuleListItem[]>("rules_list");
}

export async function setRuleEnabled(path: string, enabled: boolean): Promise<void> {
  requireBackend();
  return invoke<void>("rules_set_enabled", { path, enabled });
}
