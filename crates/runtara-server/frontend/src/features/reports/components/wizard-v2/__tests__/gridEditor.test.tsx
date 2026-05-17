import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import { GridContainer } from '../GridContainer';
import { ReportDefinition, ReportLayoutNode } from '../../../types';

function makeDefinitionWith2x2Grid(): ReportDefinition {
  return {
    definitionVersion: 1,
    filters: [],
    blocks: [
      { id: 'a', type: 'markdown', source: { schema: '' } },
      { id: 'b', type: 'markdown', source: { schema: '' } },
    ],
    layout: [
      {
        id: 'g',
        type: 'grid',
        columns: 2,
        rows: 2,
        items: [
          { id: 'item_a', child: { id: 'n_a', type: 'block', blockId: 'a' } },
          { id: 'item_b', child: { id: 'n_b', type: 'block', blockId: 'b' } },
        ],
      } as ReportLayoutNode,
    ],
  };
}

function makeEmptyGridDefinition(): ReportDefinition {
  return {
    definitionVersion: 1,
    filters: [],
    blocks: [],
    layout: [
      {
        id: 'g',
        type: 'grid',
        columns: 2,
        rows: 3,
        items: [],
      } as ReportLayoutNode,
    ],
  };
}

describe('GridContainer skeleton', () => {
  it('renders empty placeholder cells when grid has fewer items than columns × rows', () => {
    const onChange = vi.fn();
    render(
      <GridContainer
        definition={makeDefinitionWith2x2Grid()}
        schemas={[]}
        filters={{}}
        onChange={onChange}
      />
    );
    // 2x2 grid with 2 items → 2 empty placeholders.
    const empties = screen.getAllByTestId('empty-cell-g');
    expect(empties).toHaveLength(2);
  });

  it('renders the full skeleton when grid has no items at all', () => {
    const onChange = vi.fn();
    render(
      <GridContainer
        definition={makeEmptyGridDefinition()}
        schemas={[]}
        filters={{}}
        onChange={onChange}
      />
    );
    // 2x3 = 6 empty cells.
    const empties = screen.getAllByTestId('empty-cell-g');
    expect(empties).toHaveLength(6);
  });

  it('"Add column" stepper button bumps grid.columns', () => {
    const onChange = vi.fn();
    render(
      <GridContainer
        definition={makeDefinitionWith2x2Grid()}
        schemas={[]}
        filters={{}}
        onChange={onChange}
      />
    );
    fireEvent.click(screen.getByLabelText('Add columns'));
    expect(onChange).toHaveBeenCalledOnce();
    const next = onChange.mock.calls[0][0] as ReportDefinition;
    const grid = next.layout?.[0];
    if (grid?.type !== 'grid') throw new Error('expected grid');
    expect(grid.columns).toBe(3);
    // columnWidths should be cleared since the column count changed.
    expect(grid.columnWidths ?? undefined).toBeUndefined();
  });

  it('"Remove column" stepper bumps grid.columns down', () => {
    const onChange = vi.fn();
    render(
      <GridContainer
        definition={makeDefinitionWith2x2Grid()}
        schemas={[]}
        filters={{}}
        onChange={onChange}
      />
    );
    fireEvent.click(screen.getByLabelText('Remove columns'));
    const next = onChange.mock.calls[0][0] as ReportDefinition;
    const grid = next.layout?.[0];
    if (grid?.type !== 'grid') throw new Error('expected grid');
    expect(grid.columns).toBe(1);
  });

  it('"Add row" bumps grid.rows', () => {
    const onChange = vi.fn();
    render(
      <GridContainer
        definition={makeDefinitionWith2x2Grid()}
        schemas={[]}
        filters={{}}
        onChange={onChange}
      />
    );
    fireEvent.click(screen.getByLabelText('Add rows'));
    const next = onChange.mock.calls[0][0] as ReportDefinition;
    const grid = next.layout?.[0];
    if (grid?.type !== 'grid') throw new Error('expected grid');
    expect(grid.rows).toBe(3);
  });

  it('"Remove row" cannot go below the rows needed to fit current items', () => {
    const onChange = vi.fn();
    // 2 items in 2 columns → naturalRows = 1, so rows = max(rows ?? 1, 1).
    // With rows=2, Remove row drops to 1; clicking again should be disabled.
    render(
      <GridContainer
        definition={makeDefinitionWith2x2Grid()}
        schemas={[]}
        filters={{}}
        onChange={onChange}
      />
    );
    const removeRowButton = screen.getByLabelText(
      'Remove rows'
    ) as HTMLButtonElement;
    fireEvent.click(removeRowButton);
    expect(onChange).toHaveBeenCalledOnce();
    const after = onChange.mock.calls[0][0] as ReportDefinition;
    const grid = after.layout?.[0];
    if (grid?.type !== 'grid') throw new Error('expected grid');
    expect(grid.rows).toBe(1);
  });

  it('clicking "+ Add block" in an empty cell adds a block to the grid', () => {
    const onChange = vi.fn();
    render(
      <GridContainer
        definition={makeEmptyGridDefinition()}
        schemas={[]}
        filters={{}}
        onChange={onChange}
      />
    );
    // Click the first empty cell's "Add block" affordance.
    const addButtons = screen.getAllByRole('button', { name: /^Add block$/i });
    // First few buttons are the in-cell affordances; click the first.
    fireEvent.click(addButtons[0]);
    expect(onChange).toHaveBeenCalledOnce();
    const next = onChange.mock.calls[0][0] as ReportDefinition;
    expect(next.blocks).toHaveLength(1);
    expect(next.blocks[0].type).toBe('markdown');
    const grid = next.layout?.[0];
    if (grid?.type !== 'grid') throw new Error('expected grid');
    expect(grid.items).toHaveLength(1);
  });
});
