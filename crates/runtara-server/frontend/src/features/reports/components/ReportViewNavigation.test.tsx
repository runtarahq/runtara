import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import type { ReportDefinition, ReportViewNavigationState } from '../types';
import { ReportViewNavigation } from './ReportViewNavigation';

function definitionWithGroup(mode: 'tabs' | 'stages'): ReportDefinition {
  return {
    definitionVersion: 1,
    layout: { id: 'root', items: [] },
    filters: [],
    blocks: [],
    views: [
      { id: 'stage_a', title: 'Stage A', layout: { id: 'a', items: [] } },
      { id: 'stage_b', title: 'Stage B', layout: { id: 'b', items: [] } },
      { id: 'stage_c', title: 'Stage C', layout: { id: 'c', items: [] } },
    ],
    viewGroups:
      mode === 'tabs'
        ? [
            {
              id: 'details',
              mode: 'tabs',
              viewIds: ['stage_a', 'stage_b', 'stage_c'],
            },
          ]
        : [
            {
              id: 'approval',
              mode: 'stages',
              stages: [
                { viewId: 'stage_a', value: 'A' },
                { viewId: 'stage_b', value: 'B' },
                { viewId: 'stage_c', value: 'C' },
              ],
              currentFrom: { type: 'filter', filterId: 'stage' },
              access: 'through_current',
              showPreviousNext: true,
            },
          ],
  } as ReportDefinition;
}

describe('ReportViewNavigation', () => {
  it('renders peer detail views as tabs and navigates without replacing history', async () => {
    const user = userEvent.setup();
    const onNavigateView = vi.fn();
    const navigation: ReportViewNavigationState = {
      activeViewId: 'stage_a',
      group: {
        id: 'details',
        mode: 'tabs',
        accessibleViewIds: ['stage_a', 'stage_b', 'stage_c'],
      },
    };

    render(
      <ReportViewNavigation
        definition={definitionWithGroup('tabs')}
        navigation={navigation}
        activeViewId="stage_a"
        onNavigateView={onNavigateView}
      />
    );

    expect(screen.getByRole('tab', { name: 'Stage A' })).toHaveAttribute(
      'data-state',
      'active'
    );
    await user.click(screen.getByRole('tab', { name: 'Stage B' }));
    expect(onNavigateView).toHaveBeenCalledWith('stage_b', { replace: false });
  });

  it('keeps completed/current stages available and locks future stages', () => {
    const navigation: ReportViewNavigationState = {
      activeViewId: 'stage_b',
      group: {
        id: 'approval',
        mode: 'stages',
        currentViewId: 'stage_b',
        accessibleViewIds: ['stage_a', 'stage_b'],
      },
    };

    render(
      <ReportViewNavigation
        definition={definitionWithGroup('stages')}
        navigation={navigation}
        activeViewId="stage_b"
      />
    );

    expect(screen.getByRole('button', { name: /Stage A/ })).toBeEnabled();
    expect(screen.getByRole('button', { name: /Stage B/ })).toHaveAttribute(
      'aria-current',
      'step'
    );
    expect(screen.getByRole('button', { name: /Stage C/ })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Next stage' })).toBeDisabled();
    expect(screen.getByText('2 of 3')).toBeInTheDocument();
  });

  it('uses next/previous controls only within the accessible stage range', async () => {
    const user = userEvent.setup();
    const onNavigateView = vi.fn();
    const navigation: ReportViewNavigationState = {
      activeViewId: 'stage_a',
      group: {
        id: 'approval',
        mode: 'stages',
        currentViewId: 'stage_b',
        accessibleViewIds: ['stage_a', 'stage_b'],
      },
    };

    render(
      <ReportViewNavigation
        definition={definitionWithGroup('stages')}
        navigation={navigation}
        activeViewId="stage_a"
        onNavigateView={onNavigateView}
      />
    );

    expect(
      screen.getByRole('button', { name: 'Previous stage' })
    ).toBeDisabled();
    expect(screen.getByText('Viewing · Completed')).toBeInTheDocument();
    expect(screen.getByText('1 of 3')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Next stage' }));
    expect(onNavigateView).toHaveBeenCalledWith('stage_b', { replace: false });
  });

  it('configures stage button visibility and labels independently', async () => {
    const user = userEvent.setup();
    const onNavigateView = vi.fn();
    const definition = definitionWithGroup('stages');
    definition.viewGroups = [
      {
        ...definition.viewGroups![0],
        showPreviousNext: true,
        showPrevious: false,
        showNext: true,
        previousLabel: 'Back',
        nextLabel: 'Continue',
      },
    ];
    const navigation: ReportViewNavigationState = {
      activeViewId: 'stage_a',
      group: {
        id: 'approval',
        mode: 'stages',
        currentViewId: 'stage_b',
        accessibleViewIds: ['stage_a', 'stage_b'],
      },
    };

    render(
      <ReportViewNavigation
        definition={definition}
        navigation={navigation}
        activeViewId="stage_a"
        onNavigateView={onNavigateView}
      />
    );

    expect(screen.queryByRole('button', { name: 'Back' })).toBeNull();
    expect(screen.queryByRole('button', { name: 'Previous stage' })).toBeNull();
    await user.click(screen.getByRole('button', { name: 'Continue' }));
    expect(onNavigateView).toHaveBeenCalledWith('stage_b', { replace: false });
  });

  it('renders a field-controlled current stage without user switching', () => {
    const definition = definitionWithGroup('stages');
    definition.viewGroups = [
      {
        ...definition.viewGroups![0],
        access: 'current_only',
        showPrevious: false,
        showNext: false,
      },
    ];
    const navigation: ReportViewNavigationState = {
      activeViewId: 'stage_b',
      group: {
        id: 'approval',
        mode: 'stages',
        currentViewId: 'stage_b',
        accessibleViewIds: ['stage_b'],
      },
    };

    render(
      <ReportViewNavigation
        definition={definition}
        navigation={navigation}
        activeViewId="stage_b"
        onNavigateView={vi.fn()}
      />
    );

    expect(screen.getByRole('button', { name: /Stage A/ })).toBeDisabled();
    expect(screen.getByRole('button', { name: /Stage B/ })).toBeDisabled();
    expect(screen.getByRole('button', { name: /Stage B/ })).toHaveAttribute(
      'aria-current',
      'step'
    );
    expect(screen.getByRole('button', { name: /Stage C/ })).toBeDisabled();
    expect(screen.queryByRole('button', { name: 'Previous stage' })).toBeNull();
    expect(screen.queryByRole('button', { name: 'Next stage' })).toBeNull();
  });
});
