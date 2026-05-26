import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router';
import { BaseNode } from './BaseNode';

function renderNode(props: Parameters<typeof BaseNode>[0] = {}) {
  return render(
    <MemoryRouter>
      <BaseNode {...props} />
    </MemoryRouter>
  );
}

describe('BaseNode — stale-agent badge (Phase 4.6)', () => {
  it('shows the badge when hasStaleAgent is true', () => {
    renderNode({
      name: 'Fetch user',
      stepType: 'Agent',
      agentId: 'openai',
      hasStaleAgent: true,
    });
    const badge = screen.getByTestId('stale-agent-badge');
    expect(badge).toBeInTheDocument();
    expect(badge).toHaveAttribute(
      'title',
      "Agent disabled — workflow can't be saved"
    );
  });

  it('hides the badge when hasStaleAgent is false', () => {
    renderNode({
      name: 'Fetch user',
      stepType: 'Agent',
      agentId: 'http',
      hasStaleAgent: false,
    });
    expect(screen.queryByTestId('stale-agent-badge')).not.toBeInTheDocument();
  });

  it('hides the badge when hasStaleAgent is unset (default)', () => {
    // Backwards-compat: existing call sites that haven't been updated must
    // still render correctly without the prop.
    renderNode({ name: 'Fetch user', stepType: 'Agent' });
    expect(screen.queryByTestId('stale-agent-badge')).not.toBeInTheDocument();
  });
});
