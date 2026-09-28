import { describe, expect, it, vi } from 'vitest';
import { render } from '@testing-library/react';
import { MemoryRouter } from 'react-router';
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
