import { render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import type { WorkflowDto } from '@/generated/RuntaraRuntimeApi';
import { Table, TableBody } from '@/shared/components/ui/table';
import { useAuthStore } from '@/shared/stores/authStore';
import { WorkflowCard } from './index';

function buildWorkflow(overrides: Partial<WorkflowDto> = {}): WorkflowDto {
  return {
    id: 'wf-1',
    name: 'Probe workflow',
    description: '',
    created: '2026-07-27T10:00:00.000Z',
    updated: '2026-07-27T10:00:00.000Z',
    currentVersionNumber: 1,
    lastVersionNumber: 1,
    executionGraph: {},
    inputSchema: {},
    outputSchema: {},
    path: '/',
    ...overrides,
  } as unknown as WorkflowDto;
}

function renderCard(workflow: WorkflowDto, onChat = vi.fn()) {
  render(
    <Table>
      <TableBody>
        <WorkflowCard
          workflow={workflow}
          onUpdate={vi.fn()}
          onDelete={vi.fn()}
          onSchedule={vi.fn()}
          onClone={vi.fn()}
          onChat={onChat}
        />
      </TableBody>
    </Table>
  );
}

beforeEach(() => {
  // No resolved permission set — `useHasPermission` then allows everything, so
  // these cases isolate the capability gate from the permission gate.
  useAuthStore.getState().clearMe();
});

describe('WorkflowCard chat action', () => {
  it('offers Chat on a workflow that can hold a conversation', () => {
    renderCard(buildWorkflow({ supportsChat: true }));

    expect(screen.getByTitle('Chat')).toBeInTheDocument();
  });

  it('withholds Chat from a workflow with no step that waits for a reply', () => {
    renderCard(buildWorkflow({ supportsChat: false }));

    // The other row actions still render, so the absence below is the gate
    // rather than a card that failed to render at all.
    expect(screen.getByTitle('Start')).toBeInTheDocument();
    expect(screen.getByTitle('Edit')).toBeInTheDocument();
    expect(screen.queryByTitle('Chat')).not.toBeInTheDocument();
  });

  it('withholds Chat when the server did not report the capability', () => {
    // A response predating `supportsChat` must not be read as "yes".
    renderCard(buildWorkflow());

    expect(screen.queryByTitle('Chat')).not.toBeInTheDocument();
  });
});

describe('WorkflowCard chat action permissions', () => {
  it('withholds Chat from a caller who cannot execute workflows', () => {
    // Opening chat queues a run, so a role that cannot Start cannot chat
    // either — the server rejects the session POST on the same permission.
    useAuthStore.getState().setMe({
      role: 'viewer',
      permissions: { 'workflow:read': 'allow' },
    });

    renderCard(buildWorkflow({ supportsChat: true }));

    expect(screen.queryByTitle('Chat')).not.toBeInTheDocument();
    expect(screen.queryByTitle('Start')).not.toBeInTheDocument();
  });

  it('offers Chat to a caller who may execute workflows', () => {
    useAuthStore.getState().setMe({
      role: 'member',
      permissions: { 'workflow:read': 'allow', 'workflow:execute': 'allow' },
    });

    renderCard(buildWorkflow({ supportsChat: true }));

    expect(screen.getByTitle('Chat')).toBeInTheDocument();
    expect(screen.getByTitle('Start')).toBeInTheDocument();
    // Execute alone does not unlock the edit-class actions.
    expect(screen.queryByTitle('Edit')).not.toBeInTheDocument();
  });
});
