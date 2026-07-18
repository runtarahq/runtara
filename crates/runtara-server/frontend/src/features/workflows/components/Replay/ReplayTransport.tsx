/**
 * Shared transport bar for replay: play/pause, restart, scrubber (with parked-gap
 * markers), speed, pacing (even vs real-time), idle-gap compression, and an
 * elapsed/total + concurrency readout.
 */
import { Pause, Play, RotateCcw, Zap } from 'lucide-react';
import { cn } from '@/lib/utils';
import { Button } from '@/shared/components/ui/button';
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from '@/shared/components/ui/tooltip';
import { REPLAY_SPEEDS, type ReplayClock, type ReplaySpeed } from './useReplayClock';
import type { ReplayPacing, TimeMap } from './timeMap';

function formatMs(ms: number): string {
  if (!Number.isFinite(ms) || ms < 0) ms = 0;
  if (ms < 1000) return `${Math.round(ms)}ms`;
  const s = ms / 1000;
  if (s < 60) return `${s.toFixed(s < 10 ? 1 : 0)}s`;
  const m = Math.floor(s / 60);
  const rem = Math.round(s % 60);
  return `${m}m ${String(rem).padStart(2, '0')}s`;
}

interface ReplayTransportProps {
  clock: ReplayClock;
  displayEnd: number;
  modelT: number;
  totalModelMs: number;
  pacing: ReplayPacing;
  onPacingChange: (p: ReplayPacing) => void;
  compressIdle: boolean;
  onToggleCompress: (v: boolean) => void;
  runningCount: number;
  gaps: TimeMap['gaps'];
}

export function ReplayTransport({
  clock,
  displayEnd,
  modelT,
  totalModelMs,
  pacing,
  onPacingChange,
  compressIdle,
  onToggleCompress,
  runningCount,
  gaps,
}: ReplayTransportProps) {
  const pct = displayEnd > 0 ? (clock.displayT / displayEnd) * 100 : 0;

  return (
    <TooltipProvider delayDuration={300}>
      <div
        className="flex flex-wrap items-center gap-2 rounded-md border bg-background/95 px-2 py-1.5 shadow-sm backdrop-blur"
        data-testid="replay-transport"
      >
        {/* Play / Pause */}
        <Button
          type="button"
          size="icon"
          variant="default"
          className="h-7 w-7"
          onClick={clock.toggle}
          aria-label={clock.playing ? 'Pause replay' : 'Play replay'}
          data-testid="replay-play-pause"
          data-playing={clock.playing}
        >
          {clock.playing ? (
            <Pause className="h-3.5 w-3.5" />
          ) : (
            <Play className="h-3.5 w-3.5" />
          )}
        </Button>

        {/* Restart */}
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              type="button"
              size="icon"
              variant="ghost"
              className="h-7 w-7"
              onClick={clock.restart}
              aria-label="Restart replay"
              data-testid="replay-restart"
            >
              <RotateCcw className="h-3.5 w-3.5" />
            </Button>
          </TooltipTrigger>
          <TooltipContent>Restart</TooltipContent>
        </Tooltip>

        {/* Scrubber with parked-gap markers */}
        <div className="relative flex-1 min-w-[140px]">
          <div className="pointer-events-none absolute inset-x-0 top-1/2 h-1 -translate-y-1/2 overflow-hidden rounded-full bg-muted">
            <div className="h-full bg-primary/60" style={{ width: `${pct}%` }} />
            {gaps.map((g, i) => {
              const left = displayEnd > 0 ? (g.displayStart / displayEnd) * 100 : 0;
              const width =
                displayEnd > 0
                  ? ((g.displayEnd - g.displayStart) / displayEnd) * 100
                  : 0;
              return (
                <div
                  key={i}
                  className="absolute top-0 h-full bg-amber-400/50"
                  style={{ left: `${left}%`, width: `${Math.max(width, 0.6)}%` }}
                  title={`parked · ${formatMs(g.modelDurationMs)}`}
                />
              );
            })}
          </div>
          <input
            type="range"
            min={0}
            max={Math.max(displayEnd, 1)}
            step={1}
            value={clock.displayT}
            onChange={(e) => clock.seek(Number(e.target.value))}
            aria-label="Replay position"
            data-testid="replay-scrubber"
            className="relative z-10 w-full cursor-pointer appearance-none bg-transparent accent-primary [&::-webkit-slider-thumb]:h-3 [&::-webkit-slider-thumb]:w-3 [&::-webkit-slider-thumb]:appearance-none [&::-webkit-slider-thumb]:rounded-full [&::-webkit-slider-thumb]:bg-primary [&::-webkit-slider-thumb]:shadow"
          />
        </div>

        {/* Elapsed / total */}
        <div className="tabular-nums text-[11px] text-muted-foreground whitespace-nowrap">
          {formatMs(modelT)} / {formatMs(totalModelMs)}
        </div>

        {/* Concurrency indicator */}
        {runningCount > 1 && (
          <span
            className="inline-flex items-center gap-1 rounded-full bg-blue-100 px-1.5 py-0.5 text-[10px] font-medium text-blue-700 dark:bg-blue-900 dark:text-blue-300"
            data-testid="replay-running-count"
            title={`${runningCount} branches running concurrently`}
          >
            <Zap className="h-2.5 w-2.5" />
            {runningCount} running
          </span>
        )}

        {/* Speed */}
        <div className="flex items-center rounded-md border p-0.5" role="group" aria-label="Playback speed">
          {REPLAY_SPEEDS.map((s) => (
            <button
              key={s}
              type="button"
              onClick={() => clock.setSpeed(s as ReplaySpeed)}
              data-testid={`replay-speed-${s}`}
              className={cn(
                'rounded px-1.5 py-0.5 text-[10px] font-medium transition-colors',
                clock.speed === s
                  ? 'bg-primary text-primary-foreground'
                  : 'text-muted-foreground hover:bg-muted'
              )}
            >
              {s}×
            </button>
          ))}
        </div>

        {/* Pacing */}
        <div className="flex items-center rounded-md border p-0.5" role="group" aria-label="Pacing">
          {(['even', 'real'] as const).map((p) => (
            <Tooltip key={p}>
              <TooltipTrigger asChild>
                <button
                  type="button"
                  onClick={() => onPacingChange(p)}
                  data-testid={`replay-pacing-${p}`}
                  className={cn(
                    'rounded px-1.5 py-0.5 text-[10px] font-medium transition-colors',
                    pacing === p
                      ? 'bg-primary text-primary-foreground'
                      : 'text-muted-foreground hover:bg-muted'
                  )}
                >
                  {p === 'even' ? 'Even' : 'Real'}
                </button>
              </TooltipTrigger>
              <TooltipContent>
                {p === 'even'
                  ? 'Even pacing — each step gets equal screen time'
                  : 'Real-time pacing — true recorded durations'}
              </TooltipContent>
            </Tooltip>
          ))}
        </div>

        {/* Idle-gap compression (only meaningful in real-time pacing) */}
        {pacing === 'real' && (
          <Tooltip>
            <TooltipTrigger asChild>
              <button
                type="button"
                onClick={() => onToggleCompress(!compressIdle)}
                data-testid="replay-compress-idle"
                aria-pressed={compressIdle}
                className={cn(
                  'rounded-md border px-1.5 py-0.5 text-[10px] font-medium transition-colors',
                  compressIdle
                    ? 'bg-primary text-primary-foreground'
                    : 'text-muted-foreground hover:bg-muted'
                )}
              >
                Compress idle
              </button>
            </TooltipTrigger>
            <TooltipContent>
              Collapse long parked (suspended) gaps to a marker
            </TooltipContent>
          </Tooltip>
        )}
      </div>
    </TooltipProvider>
  );
}
