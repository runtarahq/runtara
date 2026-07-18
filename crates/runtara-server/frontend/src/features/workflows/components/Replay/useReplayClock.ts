/**
 * Play/pause/seek/speed state machine for the replay transport, driven by
 * `requestAnimationFrame`. The clock advances **display time** in `[0, displayEnd]`
 * (the caller maps display↔model time via the time-map so pacing/compression
 * stay orthogonal to playback). Honors `prefers-reduced-motion` by snapping to
 * the final frame instead of tweening.
 */
import { useCallback, useEffect, useRef, useState } from 'react';

export const REPLAY_SPEEDS = [1, 2, 4, 8] as const;
export type ReplaySpeed = (typeof REPLAY_SPEEDS)[number];

export interface ReplayClock {
  displayT: number;
  playing: boolean;
  speed: ReplaySpeed;
  atEnd: boolean;
  reducedMotion: boolean;
  play: () => void;
  pause: () => void;
  toggle: () => void;
  restart: () => void;
  seek: (displayT: number) => void;
  setSpeed: (speed: ReplaySpeed) => void;
}

function usePrefersReducedMotion(): boolean {
  const [reduced, setReduced] = useState(() => {
    if (typeof window === 'undefined' || !window.matchMedia) return false;
    return window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  });
  useEffect(() => {
    if (typeof window === 'undefined' || !window.matchMedia) return;
    const mq = window.matchMedia('(prefers-reduced-motion: reduce)');
    const onChange = () => setReduced(mq.matches);
    mq.addEventListener?.('change', onChange);
    return () => mq.removeEventListener?.('change', onChange);
  }, []);
  return reduced;
}

export function useReplayClock(displayEnd: number): ReplayClock {
  const reducedMotion = usePrefersReducedMotion();
  const [displayT, setDisplayT] = useState(0);
  const [playing, setPlaying] = useState(false);
  const [speed, setSpeedState] = useState<ReplaySpeed>(1);

  const rafRef = useRef<number | null>(null);
  const lastTsRef = useRef<number | null>(null);
  // Mirror state into refs so the rAF loop reads fresh values without resubscribing.
  const speedRef = useRef(speed);
  const displayEndRef = useRef(displayEnd);
  speedRef.current = speed;
  displayEndRef.current = displayEnd;

  const stopRaf = useCallback(() => {
    if (rafRef.current != null) cancelAnimationFrame(rafRef.current);
    rafRef.current = null;
    lastTsRef.current = null;
  }, []);

  // Keep the playhead within range when the display span changes (pacing toggle).
  useEffect(() => {
    setDisplayT((t) => Math.min(t, displayEnd));
  }, [displayEnd]);

  const atEnd = displayEnd > 0 && displayT >= displayEnd - 0.5;

  useEffect(() => {
    if (!playing || reducedMotion) {
      stopRaf();
      return;
    }
    const tick = (ts: number) => {
      if (lastTsRef.current == null) lastTsRef.current = ts;
      const dt = ts - lastTsRef.current;
      lastTsRef.current = ts;
      setDisplayT((prev) => {
        const next = prev + dt * speedRef.current;
        if (next >= displayEndRef.current) {
          setPlaying(false);
          return displayEndRef.current;
        }
        return next;
      });
      rafRef.current = requestAnimationFrame(tick);
    };
    rafRef.current = requestAnimationFrame(tick);
    return stopRaf;
  }, [playing, reducedMotion, stopRaf]);

  const reducedMotionRef = useRef(reducedMotion);
  reducedMotionRef.current = reducedMotion;

  const play = useCallback(() => {
    // Reduced motion: honor the preference by snapping to the final frame
    // instead of tweening. Scrubbing still works for manual stepping.
    if (reducedMotionRef.current) {
      setDisplayT(displayEndRef.current);
      return;
    }
    setDisplayT((t) => (t >= displayEndRef.current - 0.5 ? 0 : t)); // restart if at end
    setPlaying(true);
  }, []);
  const pause = useCallback(() => setPlaying(false), []);
  const toggle = useCallback(() => {
    if (reducedMotionRef.current) {
      setDisplayT((t) =>
        t >= displayEndRef.current - 0.5 ? 0 : displayEndRef.current
      );
      return;
    }
    setPlaying((p) => !p);
  }, []);
  const restart = useCallback(() => {
    if (reducedMotionRef.current) {
      setDisplayT(0);
      return;
    }
    setDisplayT(0);
    setPlaying(true);
  }, []);
  const seek = useCallback((t: number) => {
    setPlaying(false);
    setDisplayT(Math.max(0, Math.min(t, displayEndRef.current)));
  }, []);
  const setSpeed = useCallback((s: ReplaySpeed) => setSpeedState(s), []);

  return {
    displayT,
    playing,
    speed,
    atEnd,
    reducedMotion,
    play,
    pause,
    toggle,
    restart,
    seek,
    setSpeed,
  };
}
