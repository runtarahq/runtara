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

  it('clicking "+ Add block" in the bottom-right cell of a 2×3 grid pins the new block to (col=2, row=3)', () => {
    const onChange = vi.fn();
    render(
      <GridContainer
        definition={makeEmptyGridDefinition()}
        schemas={[]}
        filters={{}}
        onChange={onChange}
      />
    );
    // 2×3 grid → 6 empty cells in row-major order:
    //   index 0 = (row=1,col=1), 1 = (row=1,col=2),
    //   2 = (row=2,col=1),       3 = (row=2,col=2),
    //   4 = (row=3,col=1),       5 = (row=3,col=2).
    const addButtons = screen.getAllByRole('button', { name: /^Add block$/i });
    fireEvent.click(addButtons[5]);
    const next = onChange.mock.calls[0][0] as ReportDefinition;
    expect(next.layout.items).toHaveLength(1);
    expect(next.layout.items[0].col).toBe(2);
    expect(next.layout.items[0].row).toBe(3);
  });

  it('renders empty placeholders only at unoccupied cells when an item is pinned', () => {
    const onChange = vi.fn();
    const def: ReportDefinition = {
      definitionVersion: 1,
      filters: [],
      blocks: [{ id: 'p', type: 'markdown', source: { schema: '' } }],
      layout: {
        id: 'root',
        columns: 2,
        rows: 2,
        items: [
          {
            id: 'item_p',
            col: 1,
            row: 1,
            child: { id: 'n_p', type: 'block', blockId: 'p' },
          },
        ],
      },
    };
    render(
      <GridContainer
        definition={def}
        schemas={[]}
        filters={{}}
        onChange={onChange}
      />
    );
    // Pinned at (1,1) → 3 empty cells remain: (1,2), (2,1), (2,2).
    const empties = screen.getAllByTestId('empty-cell-root');
    expect(empties).toHaveLength(3);
  });
});

describe('GridContainer in-place editing', () => {
  it('opens the just-added block in edit mode immediately', () => {
    const onChange = vi.fn();
    let definition = makeEmptyGridDefinition();
    const { rerender } = render(
      <GridContainer
        definition={definition}
        schemas={[]}
        filters={{}}
        onChange={(next) => {
          definition = next;
          onChange(next);
        }}
      />
    );
    const addButtons = screen.getAllByRole('button', { name: /^Add block$/i });
    fireEvent.click(addButtons[0]);
    // Re-render with the new definition so the just-added block mounts.
    rerender(
      <GridContainer
        definition={definition}
        schemas={[]}
        filters={{}}
        onChange={onChange}
      />
    );
    // The new block opens the inline editor (test-id = inline-editor-<blockId>).
    expect(definition.blocks).toHaveLength(1);
    const blockId = definition.blocks[0].id;
    expect(screen.queryByTestId(`inline-editor-${blockId}`)).not.toBeNull();
  });
});
