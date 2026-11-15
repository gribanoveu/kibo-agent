import { useEffect, useMemo, useRef, useState } from "react";
import { PAUSED, sceneDocument, shouldPause } from "../lib/sceneDocument";
import { onWindowFocus } from "../lib/window";
import "./WindowScene.css";

type Props = {
  /** The scene's HTML and CSS. */
  body: string;
  /** Keep moving while another program is in use; a hidden window stands still regardless. */
  animateInBackground: boolean;
};

/**
 * The scene behind Frost's glass, in a frame that runs no script and reaches
 * no network (`sceneDocument`). It sits under the window in the page's own
 * stacking order, so the window's tint lies over it as a veil. Any motion
 * costs the system a recomposite of the window every frame, so it pauses
 * while the window is not the one in use.
 */
export function WindowScene({ body, animateInBackground }: Props) {
  const frame = useRef<HTMLIFrameElement>(null);
  const [ready, setReady] = useState(false);
  const srcDoc = useMemo(() => sceneDocument(body), [body]);

  useEffect(() => {
    let focused = document.hasFocus();
    const sync = () => {
      const root = frame.current?.contentDocument?.documentElement;
      root?.classList.toggle(PAUSED, shouldPause(document.hidden, focused, animateInBackground));
    };
    sync();
    const target = frame.current;
    target?.addEventListener("load", sync);
    const stopFocus = onWindowFocus((now) => {
      focused = now;
      sync();
    });
    document.addEventListener("visibilitychange", sync);
    return () => {
      target?.removeEventListener("load", sync);
      stopFocus();
      document.removeEventListener("visibilitychange", sync);
    };
  }, [animateInBackground, srcDoc]);

  return (
    <div className="window-scene" aria-hidden>
      <iframe
        ref={frame}
        title="Scene"
        // No `allow-scripts`: the scene is CSS. Same origin only so the window
        // can pause it; without scripts inside, that reaches nothing.
        sandbox="allow-same-origin"
        srcDoc={srcDoc}
        tabIndex={-1}
        className={ready ? "ready" : undefined}
        onLoad={() => setReady(true)}
      />
    </div>
  );
}
