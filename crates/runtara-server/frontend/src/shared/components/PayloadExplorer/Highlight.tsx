import { Fragment, type ReactNode } from 'react';
import { cn } from '@/lib/utils';

/** Most marks drawn in one string; enough for any visible row. */
const MAX_MARKS = 50;

/** Marks every case-insensitive occurrence of `query` in `text`. */
export function Highlight({
  text,
  query,
  current = false,
}: {
  text: string;
  query: string;
  /** The active search match: drawn stronger than the others. */
  current?: boolean;
}) {
  const needle = query.trim().toLowerCase();
  if (!needle) return <>{text}</>;
  const haystack = text.toLowerCase();
  const parts: ReactNode[] = [];
  let from = 0;
  let at = haystack.indexOf(needle);
  while (at !== -1 && parts.length < MAX_MARKS * 2) {
    parts.push(<Fragment key={`t${from}`}>{text.slice(from, at)}</Fragment>);
    parts.push(
      <mark
        key={`m${at}`}
        className={cn(
          'rounded-sm px-px text-inherit',
          current ? 'bg-warning/60' : 'bg-warning/25'
        )}
      >
        {text.slice(at, at + needle.length)}
      </mark>
    );
    from = at + needle.length;
    at = haystack.indexOf(needle, from);
  }
  parts.push(<Fragment key={`t${from}`}>{text.slice(from)}</Fragment>);
  return <>{parts}</>;
}
