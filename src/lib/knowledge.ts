import { invoke } from "@tauri-apps/api/core";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { inTauri, requireBackend } from "./backend";
import { onAnyToolResult } from "./chat";

// The Knowledge page (docs/40-knowledge-base.md, decision 9): the documents
// agents wrote about repositories, filed by domain. Mirrors `domain::knowledge`.

export type KnowledgeKind = "service" | "api" | "event" | "datastore" | "concept" | "algorithm" | "howto";
export type Confidence = "confirmed" | "inferred" | "unknown";

export type KnowledgeDomain = { id: string; name: string; description: string };

export type DomainCount = { domain: KnowledgeDomain; documents: number; repos: number };

export type RepoSummary = {
  name: string;
  /** Where it was open last. */
  path: string;
  commit: string | null;
  documents: number;
  stale: number;
  /** Seconds since 1970: when a document of it was last written. */
  written: number | null;
};

export type KnowledgeDocument = {
  /** `<repo>/<slug>` */
  id: string;
  domain: string;
  repo: string;
  kind: KnowledgeKind;
  title: string;
  confidence: Confidence;
  /** A file in its evidence changed since it was written. */
  stale: boolean;
  /** It cites a file at all. */
  cited: boolean;
  written: number | null;
  /** Its first paragraph, as plain text. */
  lead: string;
};

export type KnowledgeBase = { domains: DomainCount[]; repos: RepoSummary[]; documents: KnowledgeDocument[] };

export const EMPTY_BASE: KnowledgeBase = { domains: [], repos: [], documents: [] };

export async function knowledgeBase(): Promise<KnowledgeBase> {
  if (!inTauri()) return EMPTY_BASE;
  return invoke<KnowledgeBase>("knowledge_base");
}

/** A document's file as kept, frontmatter first. */
export async function knowledgeDocument(id: string): Promise<string> {
  requireBackend();
  return invoke<string>("knowledge_document", { id });
}

export async function knowledgeMove(id: string, domain: string): Promise<KnowledgeBase> {
  requireBackend();
  return invoke<KnowledgeBase>("knowledge_move", { id, domain });
}

export async function knowledgeDelete(id: string): Promise<KnowledgeBase> {
  requireBackend();
  return invoke<KnowledgeBase>("knowledge_delete", { id });
}

export async function knowledgeDeleteRepo(repo: string): Promise<KnowledgeBase> {
  requireBackend();
  return invoke<KnowledgeBase>("knowledge_delete_repo", { repo });
}

export async function knowledgeDomainEdit(domain: KnowledgeDomain): Promise<KnowledgeBase> {
  requireBackend();
  return invoke<KnowledgeBase>("knowledge_domain_edit", { domain });
}

/** Every document of `from` moved into `into`, and `from` gone. */
export async function knowledgeDomainMerge(from: string, into: string): Promise<KnowledgeBase> {
  requireBackend();
  return invoke<KnowledgeBase>("knowledge_domain_merge", { from, into });
}

/** Fires when a turn's `knowledgeWrite` kept something: the base changed. */
export function onKnowledgeChanged(handler: () => void): Promise<UnlistenFn> {
  return onAnyToolResult((result) => {
    if ((result.result as { result?: string } | undefined)?.result === "knowledgeWritten") handler();
  });
}

/** What a document's frontmatter lists, shown beside its text: the key and its items. */
export type DocumentFacts = { provides: string[]; consumes: string[]; evidence: string[]; aliases: string[] };

/**
 * A document's file split for reading: its body, and the lists in its
 * frontmatter. The frontmatter is the app's own (`knowledgeWrite` writes it),
 * so a top-level `key:` and its `- item` lines are all there is to read.
 */
export function splitDocument(file: string): { body: string; facts: DocumentFacts } {
  const facts: DocumentFacts = { provides: [], consumes: [], evidence: [], aliases: [] };
  const m = /^---\r?\n([\s\S]*?)\r?\n---\r?\n?/.exec(file);
  if (!m) return { body: file, facts };
  let key = "";
  for (const line of m[1].split(/\r?\n/)) {
    const top = /^(\w+):/.exec(line);
    if (top) key = top[1];
    const item = /^- (.+)$/.exec(line);
    if (item && key in facts) facts[key as keyof DocumentFacts].push(item[1].replace(/^(["'])(.*)\1$/, "$2"));
  }
  return { body: file.slice(m[0].length), facts };
}
