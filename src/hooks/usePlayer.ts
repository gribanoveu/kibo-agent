import { useEffect, useRef, useState } from "react";

/** A recording at `src` played in an `<audio>` the component renders with
    `audio` spread on it: where it is, whether it plays, and the moves on it.
    `missing` when there is nothing at `src` to play. */
export function usePlayer(src: string | null) {
  const ref = useRef<HTMLAudioElement>(null);
  const [playing, setPlaying] = useState(false);
  const [time, setTime] = useState(0);
  const [duration, setDuration] = useState(0);
  const [missing, setMissing] = useState(false);

  useEffect(() => {
    setPlaying(false);
    setTime(0);
    setDuration(0);
    setMissing(false);
  }, [src]);

  const seek = (at: number) => {
    if (ref.current) ref.current.currentTime = at;
    setTime(at);
  };
  // A play the element refuses (nothing loaded yet, no sound device) is told by `onError` or not at all.
  const play = () => void ref.current?.play().catch(() => {});

  return {
    audio: {
      ref,
      src: src ?? undefined,
      preload: "metadata",
      onPlay: () => setPlaying(true),
      onPause: () => setPlaying(false),
      onTimeUpdate: (e: { currentTarget: HTMLAudioElement }) => setTime(e.currentTarget.currentTime),
      onLoadedMetadata: (e: { currentTarget: HTMLAudioElement }) => setDuration(e.currentTarget.duration),
      onError: () => setMissing(true),
    },
    playing,
    time,
    duration,
    missing,
    seek,
    toggle: () => (ref.current?.paused === false ? ref.current.pause() : play()),
    /** From `at` seconds on. */
    playFrom: (at: number) => {
      seek(at);
      play();
    },
  };
}

export type Player = ReturnType<typeof usePlayer>;
