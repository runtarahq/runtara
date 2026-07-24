import { type HTMLAttributes } from 'react';
import { cn } from '@/lib/utils';

const TIER_CLASSES = {
  /** Form-section eyebrows, sidebar group titles. */
  default: 'text-xs',
  /** Dense panels (inspectors, replay, report editors). */
  sm: 'text-2xs',
  /** Densest canvas chrome. */
  xs: 'text-3xs',
} as const;

export interface SectionLabelProps extends HTMLAttributes<HTMLElement> {
  size?: keyof typeof TIER_CLASSES;
  /** Render element, defaults to <p>. */
  as?: 'p' | 'h2' | 'h3' | 'h4' | 'span' | 'div';
}

/**
 * The uppercase muted micro-label used above sections, tables, and panels.
 * One canonical spelling per tier — don't hand-roll
 * `text-3xs font-semibold uppercase tracking-…` variants.
 */
export function SectionLabel({
  size = 'default',
  as: Tag = 'p',
  className,
  ...props
}: SectionLabelProps) {
  return (
    <Tag
      className={cn(
        'font-semibold uppercase tracking-wider text-muted-foreground',
        TIER_CLASSES[size],
        className
      )}
      {...props}
    />
  );
}
