import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router';
import { WorkflowForm } from './index';

vi.mock('@/features/workflows/hooks/useFolders', () => ({
  useFolders: () => ({
    data: {
      raw: ['/Demo/Test/'],
      parsed: [
        { path: '/Demo/', name: 'Demo', parentPath: '/', depth: 1 },
        { path: '/Demo/Test/', name: 'Test', parentPath: '/Demo/', depth: 2 },
      ],
      root: [{ path: '/Demo/', name: 'Demo', parentPath: '/', depth: 1 }],
      counts: [],
    },
    isError: false,
  }),
}));

function renderForm(initialPath?: string) {
  return render(
    <MemoryRouter>
      <WorkflowForm
        title="Create workflow"
        initialPath={initialPath}
        onSubmit={vi.fn()}
      />
    </MemoryRouter>
  );
}

describe('WorkflowForm', () => {
  it('preselects the current folder in the picker', () => {
    renderForm('/Demo/Test/');
    // Radix Select renders the selected option's text inside the trigger.
    expect(screen.getByRole('combobox', { name: 'Folder' })).toHaveTextContent(
      '/Demo/Test/'
    );
  });

  it('defaults the picker to the root', () => {
    renderForm();
    expect(screen.getByRole('combobox', { name: 'Folder' })).toHaveTextContent(
      'Root (All Workflows)'
    );
  });

  it('Cancel returns to the folder the user came from', () => {
    renderForm('/Demo/Test/');
    expect(screen.getByRole('link', { name: 'Cancel' })).toHaveAttribute(
      'href',
      '/workflows?folder=%2FDemo%2FTest%2F'
    );
  });

  it('Cancel returns to the bare list from the root', () => {
    renderForm();
    expect(screen.getByRole('link', { name: 'Cancel' })).toHaveAttribute(
      'href',
      '/workflows'
    );
  });

  it('offers the folder from the URL even when it is not listed yet', () => {
    renderForm('/Brand/New/');
    expect(screen.getByRole('combobox', { name: 'Folder' })).toHaveTextContent(
      '/Brand/New/'
    );
  });
});
