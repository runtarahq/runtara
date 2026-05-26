import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { ReportsListPage } from './ReportsListPage';
import type { ReportSummary } from '../types';

const mocks = vi.hoisted(() => ({
  deleteReport: vi.fn(),
  reports: [] as ReportSummary[],
}));

vi.mock('../hooks/useReports', () => ({
  useReports: () => ({
    data: mocks.reports,
    isFetching: false,
    isError: false,
    error: null,
  }),
  useDeleteReport: () => ({
    isPending: false,
    mutateAsync: mocks.deleteReport,
  }),
}));

const sampleReport: ReportSummary = {
  id: 'rep_alpha',
  slug: 'alpha',
  name: 'Alpha report',
  description: 'Operational dashboard',
  tags: [],
  status: 'published',
  definitionVersion: 1,
  createdAt: '2026-05-14T00:00:00Z',
  updatedAt: '2026-05-14T00:00:00Z',
};

function renderPage() {
  return render(
    <MemoryRouter initialEntries={['/reports']}>
      <ReportsListPage />
    </MemoryRouter>
  );
}

describe('ReportsListPage', () => {
  beforeEach(() => {
    mocks.reports = [sampleReport];
    mocks.deleteReport.mockReset();
    mocks.deleteReport.mockResolvedValue(undefined);
  });

  it('exposes edit and delete actions from the reports list', async () => {
    renderPage();

    expect(
      screen.getByRole('button', { name: 'Edit Alpha report' })
    ).toBeInTheDocument();
    expect(
      screen.getByRole('button', { name: 'Delete Alpha report' })
    ).toBeInTheDocument();

    await userEvent.click(
      screen.getByRole('button', { name: 'Delete Alpha report' })
    );
    await userEvent.click(
      screen.getByRole('button', { name: 'Delete report' })
    );

    expect(mocks.deleteReport).toHaveBeenCalledWith('rep_alpha');
  });
});
