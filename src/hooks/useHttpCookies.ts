import { useCallback, useEffect, useState } from "react";
import { clearHttpCookies, deleteHttpCookie, listHttpCookies, type HttpCookie } from "../lib/httpCommands";

/** The HTTP page's cookie jar, read when its sheet opens: a send already going ends unseen until the next time. */
export function useHttpCookies() {
  const [cookies, setCookies] = useState<HttpCookie[] | null>(null);
  const [problem, setProblem] = useState<string | null>(null);

  const run = useCallback(async (change?: () => Promise<void>) => {
    try {
      await change?.();
      setCookies(await listHttpCookies());
      setProblem(null);
    } catch (e) {
      setProblem(`The cookies did not load: ${e}`);
    }
  }, []);

  useEffect(() => {
    void run();
  }, [run]);

  return { cookies, problem, remove: (cookie: HttpCookie) => void run(() => deleteHttpCookie(cookie)), clear: () => void run(clearHttpCookies) };
}
