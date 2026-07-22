import { describe, expect, it } from 'vitest';

import {
  ColumnSpec,
  MAX_TEXT_PX,
  MIN_TEXT_PX,
  fitColumnWidths,
  inferColumnSpecs,
} from './tableLayout';

function sum(widths: number[]): number {
  return widths.reduce((acc, w) => acc + w, 0);
}

describe('inferColumnSpecs', () => {
  it('sizes a displayField column to the displayed value, not the raw value', () => {
    const [spec] = inferColumnSpecs(
      [{ key: 'owner', displayField: 'ownerName' }],
      [
        {
          owner: '3fa85f64-5717-4562-b3fc-2c963f66afa6',
          ownerName: 'Acme',
        },
      ]
    );
    // Sized for "Acme" (floored at the text minimum) — nowhere near the
    // 30ch cap a 36-char UUID would have forced.
    expect(spec.flexible).toBe(true);
    expect(spec.idealPx).toBeLessThan(150);
    expect(spec.idealPx).toBeGreaterThanOrEqual(MIN_TEXT_PX);
  });

  it('caps a column of object values at the text maximum', () => {
    const [spec] = inferColumnSpecs(
      [{ key: 'meta' }],
      [
        {
          meta: {
            alpha: 'a-fairly-long-string-value',
            beta: 'another-long-string-value',
            gamma: 12345,
          },
        },
      ]
    );
    expect(spec.idealPx).toBeLessThanOrEqual(MAX_TEXT_PX);
  });

  it('treats maxChars as a cap, not a width promise', () => {
    const longRows = [
      { title: 'A very descriptive work item title that keeps going' },
    ];
    const shortRows = [{ title: 'abc' }];
    const column = { key: 'title', label: 'Title', maxChars: 12 };

    const [longSpec] = inferColumnSpecs([column], longRows);
    const [shortSpec] = inferColumnSpecs([column], shortRows);

    // Long data caps at maxChars(+ellipsis); short data yields a narrower
    // column instead of inheriting the cap as its width.
    expect(shortSpec.idealPx).toBeLessThan(longSpec.idealPx);
    expect(shortSpec.idealPx).toBe(MIN_TEXT_PX);
    // Cap = (12 + 3 ellipsis chars) glyphs — comfortably under 160px.
    expect(longSpec.idealPx).toBeLessThanOrEqual(160);
  });

  it('makes numeric formats rigid and floors them on the header', () => {
    const [spec] = inferColumnSpecs(
      [{ key: 'qty', label: 'A Somewhat Long Header', format: 'number' }],
      [{ qty: 7 }]
    );
    expect(spec.flexible).toBe(false);
    expect(spec.minPx).toBe(spec.idealPx);
    // Header floor: the column can never truncate its own header.
    expect(spec.idealPx).toBeGreaterThan(150);
  });

  it('falls back to format widths when there are no rows', () => {
    const [spec] = inferColumnSpecs(
      [{ key: 'qty', label: 'Qty', format: 'number' }],
      []
    );
    expect(spec.flexible).toBe(false);
    expect(spec.idealPx).toBe(104);
  });

  it('measures a formatted date column instead of using the flat width', () => {
    // The old flat table reserved 184px for any datetime column; a short
    // rendered value should now cost far less.
    const [spec] = inferColumnSpecs(
      [{ key: 'when', label: 'When', format: 'datetime' }],
      [{ when: 'Jul 3' }]
    );
    expect(spec.flexible).toBe(false);
    expect(spec.idealPx).toBeLessThan(184);
  });

  it('sizes pill columns from the humanized label within pill bounds', () => {
    const [spec] = inferColumnSpecs(
      [{ key: 'status', label: 'Status', format: 'pill' }],
      [{ status: 'in_progress' }, { status: 'done' }]
    );
    expect(spec.flexible).toBe(false);
    expect(spec.idealPx).toBeGreaterThanOrEqual(96);
    expect(spec.idealPx).toBeLessThanOrEqual(200);
  });

  it('gives action and chart columns fixed widths', () => {
    const specs = inferColumnSpecs(
      [
        {
          key: 'run',
          workflowAction: { workflow: 'wf' } as never,
        },
        { key: 'trend', type: 'chart' },
      ],
      []
    );
    for (const spec of specs) {
      expect(spec.flexible).toBe(false);
      expect(spec.idealPx).toBe(160);
    }
  });
});

describe('fitColumnWidths', () => {
  const flex = (idealPx: number, minPx: number): ColumnSpec => ({
    idealPx,
    minPx,
    flexible: true,
  });
  const rigid = (px: number): ColumnSpec => ({
    idealPx: px,
    minPx: px,
    flexible: false,
  });

  it('returns ideals when the container width is unknown', () => {
    expect(fitColumnWidths([flex(200, 100), rigid(150)], null)).toEqual([
      200, 150,
    ]);
  });

  it('returns ideals when everything fits', () => {
    expect(fitColumnWidths([flex(100, 60), flex(200, 80)], 400)).toEqual([
      100, 200,
    ]);
  });

  it('shrinks flexible columns proportionally to their headroom', () => {
    const widths = fitColumnWidths([flex(200, 100), flex(200, 100)], 300);
    expect(widths).toEqual([150, 150]);
  });

  it('never compresses below a column minimum, and overflows once all are at min', () => {
    const widths = fitColumnWidths([flex(200, 100), flex(200, 100)], 150);
    // Headroom exhausted: both at min; the remaining deficit means the table
    // legitimately scrolls.
    expect(widths).toEqual([100, 100]);
    expect(sum(widths)).toBeGreaterThan(150);
  });

  it('leaves rigid columns untouched under compression', () => {
    const widths = fitColumnWidths([rigid(150), flex(200, 100)], 250);
    expect(widths[0]).toBe(150);
    expect(widths[1]).toBe(100);
  });

  it('distributes uneven headroom proportionally', () => {
    // Headrooms 300 and 100; deficit 100 → cuts of 75 and 25.
    const widths = fitColumnWidths([flex(400, 100), flex(200, 100)], 500);
    expect(widths).toEqual([325, 175]);
    expect(sum(widths)).toBeLessThanOrEqual(500);
  });
});
