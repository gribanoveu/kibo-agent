import { useEffect, useState } from "react";
import type { PageDef, PageId } from "../components/pages";

/** A page as the registry lists it: its id is one of `PageId`, not any string. */
type ShownPage = PageDef & { id: PageId };

/**
 * The sidebar page in the chat's place: which one is open, what it was opened
 * on, and the `keep` pages mounted from their first visit on. The IDE layout
 * has no sidebar to pick a page from, so none is shown there.
 */
export function usePages(ide: boolean, pages: readonly ShownPage[]) {
  // Picking a mode or a chat leaves the page.
  const [page, setPage] = useState<PageId | null>(null);
  // What the page was opened on by another page — a meeting from Artifacts; none from the sidebar.
  const [pageItem, setPageItem] = useState<string | undefined>(undefined);
  // A feature's page only while it is on: turned off, an open one closes.
  const onPage = ide ? null : (pages.find((one) => one.id === page) ?? null);
  // The `keep` pages opened so far, mounted from then on; a feature turned off drops its page.
  const [kept, setKept] = useState<PageId[]>([]);
  const keptPages = pages.filter((one) => kept.includes(one.id) || (one === onPage && "keep" in one && one.keep));
  useEffect(() => {
    if (onPage && keptPages.includes(onPage) && !kept.includes(onPage.id)) setKept([...kept, onPage.id]);
  }, [onPage]);
  // A page gone with its feature is not kept: turned on again, it waits to be opened before it mounts.
  useEffect(() => {
    const listed = kept.filter((id) => pages.some((one) => one.id === id));
    if (listed.length !== kept.length) setKept(listed);
  }, [pages]);
  /** A page opened on something — a meeting from Artifacts; the sidebar opens one with no item. */
  const openPage = (next: PageId, item?: string) => {
    setPageItem(item);
    setPage(next);
  };
  /** Picking a mode or a chat leaves the page; what it was opened on waits for the next open. */
  const leavePage = () => setPage(null);
  return { onPage, pageItem, keptPages, openPage, leavePage };
}
