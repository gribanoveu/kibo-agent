import { useState } from "react";
import { useLiveRead } from "./useLiveRead";
import {
  EMPTY_BASE,
  knowledgeBase,
  knowledgeDelete,
  knowledgeDeleteRepo,
  knowledgeDocument,
  knowledgeDomainEdit,
  knowledgeDomainMerge,
  knowledgeMove,
  onKnowledgeChanged,
  type KnowledgeBase,
  type KnowledgeDomain,
} from "../lib/knowledge";

/**
 * The Knowledge page's state: the base, read while the page is on screen and
 * again when a turn's `knowledgeWrite` keeps something; the open document's
 * file, read the same way. A change made here answers with the base after it.
 */
export function useKnowledge(open: string | null) {
  const base = useLiveRead(knowledgeBase, onKnowledgeChanged, EMPTY_BASE);
  // The page's own change moves the document too: no signal says a move.
  const [round, setRound] = useState(0);
  const file = useLiveRead(() => (open ? knowledgeDocument(open) : Promise.resolve("")), onKnowledgeChanged, "", open !== null, `${open}#${round}`);
  const [failed, setFailed] = useState<string | null>(null);

  const change = async (work: () => Promise<KnowledgeBase>) => {
    setFailed(null);
    try {
      base.setValue(await work());
      setRound((n) => n + 1);
      return true;
    } catch (e) {
      setFailed(String(e));
      return false;
    }
  };

  return {
    base: base.value,
    error: failed ?? base.error,
    file: file.value,
    fileError: file.error,
    move: (id: string, domain: string) => change(() => knowledgeMove(id, domain)),
    remove: (id: string) => change(() => knowledgeDelete(id)),
    removeRepo: (repo: string) => change(() => knowledgeDeleteRepo(repo)),
    editDomain: (domain: KnowledgeDomain) => change(() => knowledgeDomainEdit(domain)),
    mergeDomains: (from: string, into: string) => change(() => knowledgeDomainMerge(from, into)),
  };
}
