import { useRef } from "react";
import "./ChangingText.css";

/**
 * Text that eases into a new value when it changes — a chat's name once the model has named it.
 * Not when it first appears: a list loading is not a list changing. A new `key` remounts the span,
 * which is what restarts the animation.
 */
export function ChangingText({ text }: { text: string }) {
  const first = useRef(text);
  return (
    <span key={text} className={text === first.current ? undefined : "changing-text"}>
      {text}
    </span>
  );
}
