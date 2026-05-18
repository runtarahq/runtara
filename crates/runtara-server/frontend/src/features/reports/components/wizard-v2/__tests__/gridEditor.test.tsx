import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import { GridContainer } from '../GridContainer';
import { ReportDefinition } from '../../../types';

function makeDefinitionWith2x2Grid(): ReportDefinition {
  return {
    definitionVersion: 1,
    filters: [],
    blocks: [
      { id: 'a', type: 'markdown', source: { schema: '' } },
      { id: 'b', type: 'markdown', source: { schema: '' } },
    ],
    layout: {
      id: 'root',
      columns: 2,
      rows: 2,
      items: [
        { id: 'item_a', child: { id: 'n_a', type: 'block', blockId: 'a' } },
        { id: 'item_b', child: { id: 'n_b', type: 'block', blockId: 'b' } },
      ],
    },
  };
}

function makeEmptyGridDefinition(): ReportDefinition {
  return {
    definitionVersion: 1,
    filters: [],
    blocks: [],
    layout: {
      id: 'root',
      columns: 2,
      rows: 3,
      items: [],
    },
  };
}

describe('GridContainer skeleton', () => {
  it('renders empty placeholder cells when root grid has fewer items than columns × rows', () => {
    const onChange = vi.fn();
    render(
      <GridContainer
        definition={makeDefinitionWith2x2Grid()}
        schemas={[]}
        filters={{}}
        onChange={onChange}
      />
    );
    // 2x2 root grid with 2 items → 2 empty placeholders.
    const empties = screen.getAllByTestId('empty-cell-root');
    expect(empties).toHaveLength(2);
  });

  it('renders the full skeleton when root grid has no items at all', () => {
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
    const empties = screen.getAllByTestId('empty-cell-root');
    expect(empties).toHaveLength(6);
  });

  it('"Add column" stepper bumps root.columns', () => {
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
    expect(next.layout.columns).toBe(3);
    expect(next.layout.columnWidths ?? undefined).toBeUndefined();
  });

  it('"Remove column" stepper bumps root.columns down', () => {
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
    expect(next.layout.columns).toBe(1);
  });

  it('"Add row" bumps root.rows', () => {
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
    expect(next.layout.rows).toBe(3);
  });

  it('"Remove row" floor is the rows needed to fit current items', () => {
    const onChange = vi.fn();
    render(
      <GridContainer
        definition={makeDefinitionWith2x2Grid()}
        schemas={[]}
        filters={{}}
        onChange={onChange}
      />
    );
    fireEvent.click(screen.getByLabelText('Remove rows'));
    expect(onChange).toHaveBeenCalledOnce();
    const after = onChange.mock.calls[0][0] as ReportDefinition;
    expect(after.layout.rows).toBe(1);
  });

  it('clicking "+ Add block" in an empty root cell adds a block to the root grid', () => {
    const onChange = vi.fn();
    render(
      <GridContainer
        definition={makeEmptyGridDefinition()}
        schemas={[]}
        filters={{}}
        onChange={onChange}
      />
    );
    const addButtons = screen.getAllByRole('button', { name: /^Add block$/i });
    fireEvent.click(addButtons[0]);
    expect(onChange).toHaveBeenCalledOnce();
    const next = onChange.mock.calls[0][0] as ReportDefinition;
    expect(next.blocks).toHaveLength(1);
    expect(next.blocks[0].type).toBe('markdown');
    expect(next.layout.items).toHaveLength(1);
  });
});
