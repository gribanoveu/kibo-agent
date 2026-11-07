import { useEffect, useReducer, useRef, useState, type Dispatch, type RefObject, type SetStateAction } from "react";
import { dictationCancel, dictationStart, dictationStep, dictationStop, holdKey, insertPhrase, micButton, NOT_DICTATING, type DictationEvent } from "../lib/dictation";
import { meetingMicrophoneGet, meetingMicrophones, meetingMicrophoneSet, type Microphone } from "../lib/meeting";
import { matches, released } from "../lib/shortcuts";
import { playTone, type Tone } from "../lib/sounds";
import { useRecordingAccess } from "./useRecordingAccess";

/** One dictation, from the press on. */
type Session = {
  /** The box as it was: a cancel goes back to it. */
  before: string;
  /** Where the next phrase goes. */
  at: number;
  /** The box as this dictation left it: anything else there came from outside, a
      message taken back from the queue, say, and is not to be undone. */
  wrote: string;
  /** Stop and Cancel wait for it: the service knows of no dictation until it is through. */
  started: Promise<void>;
  /** The system's prompt is up, nothing begun yet. */
  asking: boolean;
  /** Stop or Cancel was asked for: nothing more is begun or sounded. */
  stopping: boolean;
};

/** No output device, no sound: the dictation goes on. */
const tone = (t: Tone) => void playTone(t).catch(() => {});

type Options = {
  on: boolean;
  /** Whose box it is: another chat's text is not this dictation's. */
  draftKey: string;
  text: string;
  setText: Dispatch<SetStateAction<string>>;
  area: RefObject<HTMLTextAreaElement | null>;
  /** The text changed from here: the box measures itself again. */
  onChange: () => void;
};

/** Dictation into the message box (`docs/52-voice-input.md`): its button, its key and what it does to the text. */
export function useDictation({ on, draftKey, text, setText, area, onChange }: Options) {
  const { access, allow, error: accessError, clearError } = useRecordingAccess(on);
  const [view, dispatch] = useReducer(dictationStep, NOT_DICTATING);
  const [microphones, setMicrophones] = useState<Microphone[] | null>(null);
  const [microphone, setMicrophone] = useState<string | null>(null);
  const session = useRef<Session | null>(null);
  /** The key is held again while the last dictation finishes: it starts when that ends. */
  const again = useRef(false);
  // The box now, a phrase put in it included: two can come before the next render.
  const box = useRef(text);
  box.current = text;

  const hear = (me: Session, event: DictationEvent) => {
    if (session.current !== me) return;
    if (event.kind === "listening" && !me.stopping) tone("listening");
    const untouched = box.current === me.wrote;
    if (event.kind === "phrase") {
      const next = insertPhrase(box.current, untouched ? me.at : box.current.length, event.text);
      box.current = me.wrote = next.text;
      me.at = next.at;
      setText(next.text);
      onChange();
    }
    if (event.kind === "ended" || event.kind === "failed") session.current = null;
    if (event.kind === "ended" && event.reason === "cancelled") {
      if (untouched) {
        box.current = me.before;
        setText(me.before);
        onChange();
      }
    } else if (event.kind === "ended") {
      if (event.reason === "limit") tone("stopped");
      requestAnimationFrame(() => {
        area.current?.focus();
        area.current?.setSelectionRange(me.at, me.at);
      });
    }
    dispatch(event);
    // A Stop can get ahead of `preparing`, which would put the button back to Stop.
    if (event.kind === "preparing" && me.stopping) dispatch({ kind: "stopping" });
    if ((event.kind === "ended" || event.kind === "failed") && again.current) {
      again.current = false;
      void keys.current.start();
    }
  };

  const start = async () => {
    if (session.current) {
      again.current = session.current.stopping;
      return;
    }
    if (access?.microphone === "denied") {
      dispatch({ kind: "refused", message: "The app may not use the microphone — allow it in the system's settings." });
      return;
    }
    const me: Session = { before: "", at: 0, wrote: "", started: Promise.resolve(), asking: false, stopping: false };
    session.current = me;
    if (access?.microphone === "notAsked") {
      me.asking = true;
      const answer = await allow("microphone");
      me.asking = false;
      if (session.current !== me) return;
      if (answer !== "granted") {
        session.current = null;
        return;
      }
    }
    me.before = me.wrote = box.current;
    me.at = area.current?.selectionStart ?? box.current.length;
    me.started = dictationStart((event) => hear(me, event)).catch((e) => {
      if (session.current !== me) return;
      session.current = null;
      dispatch({ kind: "refused", message: String(e) });
    });
  };

  const stop = () => {
    again.current = false;
    const me = session.current;
    if (!me || me.stopping) return;
    if (me.asking) {
      session.current = null;
      return;
    }
    me.stopping = true;
    tone("stopped");
    dispatch({ kind: "stopping" });
    void me.started.then(dictationStop);
  };

  const cancel = () => {
    const me = session.current;
    if (!me) return;
    if (me.asking) {
      session.current = null;
      return;
    }
    me.stopping = true;
    void me.started.then(dictationCancel);
  };

  // Read at the moment of the key, as `useShortcuts` does.
  const keys = useRef({ start, stop, cancel });
  keys.current = { start, stop, cancel };

  // Another chat opened, the box hidden or gone: what was said stays where it
  // went, and nothing more comes. Before the key's effect, whose cleanup would
  // otherwise Stop it first.
  useEffect(
    () => () => {
      const me = session.current;
      if (!me) return;
      session.current = null;
      again.current = false;
      dispatch({ kind: "ended", reason: "cancelled" });
      void me.started.then(dictationCancel);
    },
    [draftKey, on],
  );

  useEffect(() => {
    if (!on) return;
    const hold = holdKey(
      () => void keys.current.start(),
      () => keys.current.stop(),
    );
    const onDown = (e: KeyboardEvent) => {
      // An open menu or dialog is what Escape closes: thrown away, what was said is gone.
      if (session.current && matches(e, "close") && !document.querySelector('[role="listbox"], [role="dialog"]')) {
        e.preventDefault();
        e.stopImmediatePropagation();
        keys.current.cancel();
      } else if (matches(e, "dictate")) hold.press();
      else hold.other();
    };
    const onUp = (e: KeyboardEvent) => {
      if (released(e, "dictate")) hold.lift();
    };
    const onBlur = () => hold.lift();
    window.addEventListener("keydown", onDown, true);
    window.addEventListener("keyup", onUp, true);
    window.addEventListener("blur", onBlur);
    return () => {
      hold.lift();
      window.removeEventListener("keydown", onDown, true);
      window.removeEventListener("keyup", onUp, true);
      window.removeEventListener("blur", onBlur);
    };
  }, [on]);

  const button = micButton(access?.microphone, view.phase);
  const press = () => {
    if (button.press === "start") void start();
    else if (button.press === "settings") void allow("microphone");
    else if (button.press === "stop") stop();
    else if (button.press === "cancel") cancel();
  };

  return {
    ...view,
    error: view.error ?? accessError,
    clearError: () => {
      dispatch({ kind: "dismissed" });
      clearError();
    },
    /** The text is the dictation's until it ends. */
    active: view.phase !== "idle",
    button,
    press,
    microphone,
    microphones,
    /** The menu opened: the list and the choice, which Meetings may have changed. */
    readMicrophones: () => {
      void meetingMicrophones().then(setMicrophones, (e) => dispatch({ kind: "noted", message: String(e) }));
      void meetingMicrophoneGet().then(setMicrophone, (e) => dispatch({ kind: "noted", message: String(e) }));
    },
    pickMicrophone: (id: string | null) => {
      setMicrophone(id);
      void meetingMicrophoneSet(id).catch((e) => dispatch({ kind: "noted", message: String(e) }));
    },
  };
}
