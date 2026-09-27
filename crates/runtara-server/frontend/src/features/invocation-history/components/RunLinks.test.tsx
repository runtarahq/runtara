import { describe, expect, it, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router';
import { ChildRunsCard, ParentRunLink, RunStatusPill } from './RunLinks';

const query = vi.hoisted(() => vi.fn());
vi.mock('@/shared/hooks/api', () => ({ useCustomQuery: query }));
vi.mock('../queries', () => ({ getAllExecutions: vi.fn() }));

const item = (instanceId: string, extra: Record<string, unknown> = {}) => ({
  instanceId,
  workflowId: `wf-${instanceId}`,
  workflowName: `Workflow ${instanceId}`,
  createdAt: '2026-09-27T10:00:00Z',
  status: 'completed',
  version: 1,
  ...extra,
});

function withRouter(node: React.ReactNode) {
  return render(<MemoryRouter>{node}</MemoryRouter>);
}

beforeEach(() => query.mockReset());

describe('Started by / Parent', () => {
  it('links to the parent run detail once the parent is resolved', () => {
    query.mockReturnValue({
      data: { content: [item('parent-10'), item('parent-1')] },
    });
    withRouter(<ParentRunLink parentInstanceId="parent-1" />);
    const params = query.mock.lastCall![0];
    expect(params.enabled).toBe(true);
    expect(params.queryKey.at(-1).filters).toEqual({ search: 'parent-1' });
    expect(screen.getByRole('link')).toHaveAttribute(
      'href',
      '/workflows/wf-parent-1/history/parent-1'
    );
    expect(screen.getByText('Workflow parent-1')).toBeInTheDocument();
  });

  it('shows the bare id while unresolved and a dash without a parent', () => {
    query.mockReturnValue({ data: undefined });
    const { rerender } = withRouter(
      <ParentRunLink parentInstanceId="parent-1" />
    );
    expect(screen.queryByRole('link')).not.toBeInTheDocument();
    expect(screen.getByText('parent-1')).toBeInTheDocument();

    rerender(
      <MemoryRouter>
        <ParentRunLink parentInstanceId={null} />
      </MemoryRouter>
    );
    expect(query.mock.lastCall![0].enabled).toBe(false);
    expect(screen.getByText('—')).toBeInTheDocument();
  });
});

describe('Child runs', () => {
  it('lists the children the run started, filtered by parentInstanceId', () => {
    query.mockReturnValue({
      data: {
        content: [
          item('child-1', {
            status: 'suspended',
            suspensionReason: 'waiting_signal',
          }),
        ],
        totalElements: 1,
      },
    });
    withRouter(<ChildRunsCard instanceId="parent-1" />);
    expect(query.mock.lastCall![0].queryKey.at(-1).filters).toMatchObject({
      parentInstanceId: 'parent-1',
    });
    expect(screen.getByText('Child runs')).toBeInTheDocument();
    expect(screen.getByText('Workflow child-1').closest('a')).toHaveAttribute(
      'href',
      '/workflows/wf-child-1/history/child-1'
    );
    expect(screen.getByText('Waiting for signal')).toBeInTheDocument();
  });

  it('renders nothing when the run started no children', () => {
    query.mockReturnValue({ data: { content: [], totalElements: 0 } });
    withRouter(<ChildRunsCard instanceId="parent-1" />);
    expect(screen.queryByTestId('child-runs')).not.toBeInTheDocument();
  });
});

describe('RunStatusPill', () => {
  it('names the reason for a suspended run', () => {
    const { rerender } = render(
      <RunStatusPill status="suspended" suspensionReason="paused" />
    );
    expect(screen.getByText('Paused')).toBeInTheDocument();
    rerender(<RunStatusPill status="failed" />);
    expect(screen.getByText('Failed')).toBeInTheDocument();
  });
});
