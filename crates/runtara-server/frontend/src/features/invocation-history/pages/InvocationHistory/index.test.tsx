import { InvocationHistoryRedirect } from '@/router/InvocationHistoryRedirect';
import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import { MemoryRouter, Routes, Route, useLocation } from 'react-router';
import type { ExecutionHistoryFilters } from '../../types';
import { InvocationHistory } from './index';

const table = vi.hoisted(() => vi.fn());
vi.mock('../../components/InvocationHistoryTable', () => ({
  InvocationHistoryTable: (props: {
    filters: ExecutionHistoryFilters;
    onFiltersChange: (filters: ExecutionHistoryFilters) => void;
  }) => {
    table(props);
    return null;
  },
}));
vi.mock('@/shared/hooks/usePageTitle', () => ({ usePageTitle: () => {} }));

describe('InvocationHistory page', () => {
  it('reads the parent filter from the URL', () => {
    render(
      <MemoryRouter
        initialEntries={['/invocation-history?parentInstanceId=p-1']}
      >
        <InvocationHistory />
      </MemoryRouter>
    );
    expect(table.mock.lastCall![0].filters.parentInstanceId).toBe('p-1');
  });
});

it.each(['/invocation-history', '/operations/monitor'])(
  'preserves bookmarked %s filters and fragments when redirecting to Operations Runs',
  (legacyPath) => {
    function Runs() {
      const location = useLocation();
      return (
        <>
          <output>{location.pathname + location.search + location.hash}</output>
          <InvocationHistory />
        </>
      );
    }
    render(
      <MemoryRouter
        initialEntries={[
          `${legacyPath}?parentInstanceId=p-1&workflowId=w-1&status=failed&runLabel=ORDER-123#results`,
        ]}
      >
        <Routes>
          <Route path={legacyPath} element={<InvocationHistoryRedirect />} />
          <Route path="/operations/runs" element={<Runs />} />
        </Routes>
      </MemoryRouter>
    );
    expect(
      screen.getByText(
        '/operations/runs?parentInstanceId=p-1&workflowId=w-1&status=failed&runLabel=ORDER-123#results'
      )
    ).toBeTruthy();
    expect(table.mock.lastCall![0].filters).toMatchObject({
      parentInstanceId: 'p-1',
      workflowId: 'w-1',
      status: 'failed',
      runLabel: 'ORDER-123',
    });
  }
);
