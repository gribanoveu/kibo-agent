import { WAVE_BARS } from "../lib/dictation";
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
