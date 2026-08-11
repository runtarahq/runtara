import { render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import type { WorkflowDto } from '@/generated/RuntaraRuntimeApi';
import { Table, TableBody } from '@/shared/components/ui/table';
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
