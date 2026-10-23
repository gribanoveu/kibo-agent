import type { Ref } from "react";
import "./PrototypeFrame.css";

type Props = {
  src: string;
  title: string;
  /** Where the frame sits: its size and place are the caller's. */
  className?: string;
  ref?: Ref<HTMLIFrameElement>;
};

/**
 * A page the model built, in a frame of its own (docs/31-prototype-role.md,
 * decision 4): its scripts run, and nothing else — no same origin, so no
 * reaching the window; no leaving the frame; no windows of its own. The one
 * place these rules are written, for every frame that shows such a page.
 */
export function PrototypeFrame({ src, title, className, ref }: Props) {
  return (
    <iframe
      ref={ref}
      className={`prototype-frame${className ? ` ${className}` : ""}`}
      title={title}
      src={src}
      sandbox="allow-scripts allow-modals allow-forms"
      // `*`, not the default `src`: without allow-same-origin the page's
      // origin is opaque, and WebKit matches it against `src` literally —
      // `requestFullscreen` was refused "by permissions policy".
      allow="fullscreen *"
    />
  );
}
