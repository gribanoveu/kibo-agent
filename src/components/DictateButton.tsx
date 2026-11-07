import { Loader2, Mic, MicOff, Square } from "lucide-react";
import { Dropdown } from "./Dropdown";
import { DEFAULT_MIC, micOptions } from "../lib/meeting";
import type { useDictation } from "../hooks/useDictation";
import "./DictateButton.css";

const ICONS = { mic: Mic, refused: MicOff, working: Loader2, stop: Square };

/** The microphone beside Send, and the menu of what it dictates from. */
export function DictateButton({ dictation }: { dictation: ReturnType<typeof useDictation> }) {
  const { button, microphone, microphones } = dictation;
  const Icon = ICONS[button.look];
  return (
    <div className="dictate">
      <button type="button" className={`dictate-mic ${button.look}`} title={button.title} aria-label={button.title} disabled={!button.press} onClick={dictation.press}>
        <Icon size={button.look === "stop" ? 14 : 16} />
      </button>
      <Dropdown
        title="Dictate from"
        heading="Dictate from"
        label={null}
        right
        value={microphone ?? DEFAULT_MIC}
        options={micOptions(microphone, microphones)}
        onOpen={dictation.readMicrophones}
        onPick={(id) => dictation.pickMicrophone(id === DEFAULT_MIC ? null : id)}
      />
    </div>
  );
}
