import { useState } from 'react';
import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import { ReportDefinition } from '../../../types';
import { ReportBuilderWizardV2 } from '../ReportBuilderWizardV2';
import { ViewsEditorV2 } from '../ViewsEditorV2';

function makeDefinition(): ReportDefinition {
  return {
    definitionVersion: 1,
    layout: { id: 'root', columns: 1, rows: 1, items: [] },
    filters: [{ id: 'stage', label: 'Stage', type: 'text' }],
    blocks: [],
    views: [
      {
        id: 'overview',
        title: 'Overview',
        layout: { id: 'overview-root', columns: 1, rows: 1, items: [] },
      },
      {
        id: 'review',
        title: 'Review',
        layout: { id: 'review-root', columns: 2, rows: 1, items: [] },
      },
      {
        id: 'complete',
        title: 'Complete',
        layout: { id: 'complete-root', columns: 1, rows: 1, items: [] },
      },
    ],
  };
}

function ViewsHarness({
  initial,
  onChange,
}: {
  initial: ReportDefinition;
  onChange: (definition: ReportDefinition) => void;
}) {
  const [definition, setDefinition] = useState(initial);
  return (
    <ViewsEditorV2
      definition={definition}
      onChange={(next) => {
        onChange(next);
        setDefinition(next);
      }}
    />
  );
}

function WizardHarness({
  initial,
  onChange,
}: {
  initial: ReportDefinition;
  onChange: (definition: ReportDefinition) => void;
}) {
  const [definition, setDefinition] = useState(initial);
  return (
    <ReportBuilderWizardV2
      definition={definition}
      schemas={[]}
      onChange={(next) => {
        onChange(next);
        setDefinition(next);
      }}
    />
  );
}

describe('report view authoring', () => {
  it('edits a selected detail layout without replacing the main layout', () => {
    const onChange = vi.fn();
    render(<WizardHarness initial={makeDefinition()} onChange={onChange} />);

    fireEvent.change(screen.getByLabelText('Layout to edit'), {
      target: { value: 'review' },
    });
    expect(screen.getByTestId('grid-review-root')).toBeInTheDocument();
    fireEvent.click(screen.getByLabelText('Add columns'));

    const next = onChange.mock.lastCall?.[0] as ReportDefinition;
    expect(next.layout.columns).toBe(1);
    expect(
      next.views?.find((view) => view.id === 'review')?.layout?.columns
    ).toBe(3);
  });

  it('authors ordered stage navigation with a filter-backed current stage', () => {
    const onChange = vi.fn();
    render(<ViewsHarness initial={makeDefinition()} onChange={onChange} />);

    fireEvent.click(screen.getByRole('button', { name: 'Add stage group' }));
    let next = onChange.mock.lastCall?.[0] as ReportDefinition;
    expect(next.viewGroups?.[0]).toMatchObject({
      mode: 'stages',
      access: 'through_current',
      currentFrom: { type: 'filter', filterId: 'stage' },
      followCurrentOnAdvance: true,
      showPrevious: true,
      showNext: true,
      stages: [
        { viewId: 'overview', value: 'overview' },
        { viewId: 'review', value: 'review' },
      ],
    });

    fireEvent.change(screen.getByLabelText('Previous label'), {
      target: { value: 'Back' },
    });
    fireEvent.change(screen.getByLabelText('Next label'), {
      target: { value: 'Continue' },
    });
    fireEvent.click(screen.getByLabelText('Show Next button'));
    next = onChange.mock.lastCall?.[0] as ReportDefinition;
    expect(next.viewGroups?.[0]).toMatchObject({
      showPrevious: true,
      showNext: false,
      previousLabel: 'Back',
      nextLabel: 'Continue',
    });

    fireEvent.click(screen.getByRole('button', { name: 'Add member' }));
    fireEvent.click(screen.getByLabelText('Move member 3 up'));
    next = onChange.mock.lastCall?.[0] as ReportDefinition;
    expect(next.viewGroups?.[0].stages?.map((stage) => stage.viewId)).toEqual([
      'overview',
      'complete',
      'review',
    ]);
  });

  it('authors a field-controlled stage group and hides switching controls', () => {
    const onChange = vi.fn();
    const definition = makeDefinition();
    definition.blocks = [
      {
        id: 'case_state',
        type: 'markdown',
        source: { schema: '' },
      },
    ];
    render(<ViewsHarness initial={definition} onChange={onChange} />);

    fireEvent.click(screen.getByRole('button', { name: 'Add stage group' }));
    fireEvent.change(screen.getByLabelText('Accessible views'), {
      target: { value: 'current_only' },
    });

    const next = onChange.mock.lastCall?.[0] as ReportDefinition;
    expect(next.viewGroups?.[0]).toMatchObject({
      access: 'current_only',
      currentFrom: {
        type: 'block',
        blockId: 'case_state',
        field: 'status',
      },
      showPrevious: false,
      showNext: false,
    });
    expect(
      screen.getByText(
        'The current stage comes from the configured field. Viewers cannot switch stages.'
      )
    ).toBeInTheDocument();
  });

  it('updates parent and navigation references when a view id changes', () => {
    const initial: ReportDefinition = {
      ...makeDefinition(),
      views: makeDefinition().views?.map((view) =>
        view.id === 'review' ? { ...view, parentViewId: 'overview' } : view
      ),
      viewGroups: [
        {
          id: 'details',
          mode: 'tabs',
          viewIds: ['overview', 'review'],
        },
      ],
    };
    const onChange = vi.fn();
    render(<ViewsHarness initial={initial} onChange={onChange} />);

    fireEvent.change(screen.getAllByLabelText('ID')[0], {
      target: { value: 'summary' },
    });
    const next = onChange.mock.lastCall?.[0] as ReportDefinition;
    expect(next.views?.find((view) => view.id === 'review')?.parentViewId).toBe(
      'summary'
    );
    expect(next.viewGroups?.[0].viewIds).toEqual(['summary', 'review']);
  });
});
