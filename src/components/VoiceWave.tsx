import { WAVE_BARS, type DictationView } from "../lib/dictation";
import "./VoiceWave.css";

/** How loud it has been, newest at the right; the empty bars keep it from growing as it starts. */
export function VoiceWave({ peaks }: { peaks: number[] }) {
  const bars = [...Array<number>(WAVE_BARS - peaks.length).fill(0), ...peaks];
  return (
    <div className="voice-wave" aria-hidden="true">
      {/* A square root: speech peaks low, and a wave that barely moves says nothing. */}
      {bars.map((peak, i) => (
        <span key={i} style={{ height: `${Math.max(6, Math.round(100 * Math.sqrt(peak)))}%` }} />
      ))}
    </div>
  );
}

/** Under the box while it dictates. The phrase being said changes until it is whole, and then goes in the box. */
export function DictationLine({ peaks, partial }: Pick<DictationView, "peaks" | "partial">) {
  return (
    <div className="dictation-line">
      <VoiceWave peaks={peaks} />
      {partial && <p className="dictation-partial chat-text">{partial}</p>}
    </div>
  );
}
