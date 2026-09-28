import { describe, expect, it } from 'vitest';

import {
  RANGE_PRESETS,
  describeRange,
  fromLocalInput,
  matchingPreset,
  presetRange,
  rangeFormat,
  toLocalInput,
} from './date-values';
import type { FormField } from './types';

const preset = (label: string) => RANGE_PRESETS.find((p) => p.label === label)!;
// Local noon on 15 March 2026, so no preset crosses a daylight-saving change.
const now = new Date(2026, 2, 15, 12, 0);
const range = (format: string): FormField => ({
  type: 'object',
  properties: {
    from: { type: 'string', format },
    to: { type: 'string', format },
  },
});

describe('date values', () => {
  it('picks date-times in local time and stores them in UTC', () => {
    const stored = fromLocalInput('2026-03-15T09:30');
    expect(stored).toBe(new Date(2026, 2, 15, 9, 30).toISOString());
    expect(toLocalInput(stored)).toBe('2026-03-15T09:30');
    expect(toLocalInput('not a date')).toBe('');
    expect(fromLocalInput('')).toBe('');
  });

  it('recognizes a from/to object of dates or date-times as a range', () => {
    expect(rangeFormat(range('date'))).toBe('date');
    expect(rangeFormat(range('datetime'))).toBe('datetime');
    expect(rangeFormat(range('email'))).toBeUndefined();
    expect(
      rangeFormat({
        type: 'object',
        properties: {
          from: { type: 'string', format: 'date' },
          to: { type: 'string', format: 'datetime' },
        },
      })
    ).toBeUndefined();
    expect(
      rangeFormat({
        type: 'object',
        properties: { ...range('date').properties, step: { type: 'string' } },
      })
    ).toBeUndefined();
  });

  it('gives date presets inclusive days and date-time presets day starts', () => {
    expect(presetRange(preset('Last 7 days'), 'date', now)).toEqual({
      from: '2026-03-09',
      to: '2026-03-15',
    });
    expect(presetRange(preset('Last month'), 'date', now)).toEqual({
      from: '2026-02-01',
      to: '2026-02-28',
    });
    expect(presetRange(preset('Today'), 'datetime', now)).toEqual({
      from: new Date(2026, 2, 15).toISOString(),
      to: new Date(2026, 2, 16).toISOString(),
    });
  });

  it('names a range by its preset, or by its bounds', () => {
    const week = presetRange(preset('Last 7 days'), 'datetime', now);
    expect(matchingPreset(week, 'datetime', now)?.label).toBe('Last 7 days');
    expect(describeRange(week, 'datetime', now)).toBe('Last 7 days');
    // The same instants written differently still match.
    expect(
      matchingPreset(
        { from: week.from.replace('.000Z', 'Z'), to: week.to },
        'datetime',
        now
      )?.label
    ).toBe('Last 7 days');
    expect(describeRange({}, 'date', now)).toBe('Choose a period');
    expect(
      describeRange({ from: '2026-01-02', to: '2026-01-05' }, 'date', now)
    ).toContain('–');
  });
});
