import { describe, expect, it } from 'vitest';
import {
  COLUMN_MIN_VIEWPORT,
  isColumnVisibleAt,
  responsiveColumnClass,
  shortRunId,
  type InvocationColumnId,
} from './column-layout';

const ALL: InvocationColumnId[] = [
  'workflowId',
  'createdAt',
  'completedAt',
  'status',
  'parentInstanceId',
  'executionDurationSeconds',
  'version',
  'actions',
];
const visibleAt = (width: number) =>
  ALL.filter((id) => isColumnVisibleAt(id, width));

describe('invocation history column layout', () => {
  it('always shows Execution, Started, Status and Actions', () => {
    expect(visibleAt(1024)).toEqual([
      'workflowId',
      'createdAt',
      'status',
      'actions',
    ]);
    for (const id of ['workflowId', 'createdAt', 'status', 'actions'] as const)
      expect(responsiveColumnClass(id)).toBe('');
  });

  it('adds lower-priority columns as the viewport widens', () => {
    expect(visibleAt(1280)).toEqual([
      'workflowId',
      'createdAt',
      'status',
      'executionDurationSeconds',
      'actions',
    ]);
    expect(visibleAt(1440)).toContain('parentInstanceId');
    expect(visibleAt(1440)).not.toContain('completedAt');
    // 1600px: everything except Version, so Actions stays on screen.
    expect(visibleAt(1600)).toEqual(ALL.filter((id) => id !== 'version'));
    expect(visibleAt(1800)).toEqual(ALL);
  });

  it('hides each responsive column below its breakpoint with display classes', () => {
    expect(responsiveColumnClass('executionDurationSeconds')).toBe(
      'hidden xl:table-cell'
    );
    expect(responsiveColumnClass('parentInstanceId')).toBe(
      'hidden min-[1440px]:table-cell'
    );
    expect(responsiveColumnClass('completedAt')).toBe(
      'hidden min-[1600px]:table-cell'
    );
    expect(responsiveColumnClass('version')).toBe(
      'hidden min-[1800px]:table-cell'
    );
    // Every breakpoint has a class, or the column would never hide.
    for (const id of Object.keys(COLUMN_MIN_VIEWPORT) as InvocationColumnId[])
      expect(responsiveColumnClass(id)).toMatch(/^hidden /);
  });

  it('shortens run ids for compact cells', () => {
    expect(shortRunId('77c7a528-0dba-4afa-9680-d5112c38bf1c')).toBe(
      '77c7a528…'
    );
    expect(shortRunId('run-1')).toBe('run-1');
  });
});
