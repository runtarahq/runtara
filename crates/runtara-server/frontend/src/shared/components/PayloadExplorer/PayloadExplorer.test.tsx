import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { toast } from 'sonner';
import { PayloadExplorer, type PayloadSection } from './index';

vi.mock('sonner', () => ({
  toast: { success: vi.fn(), error: vi.fn(), info: vi.fn() },
}));

const elided = {
  _elided: true,
  _original_size: 1_843_200,
  _preview: 'JVBERi0x',
};

function runSections(
  overrides: Partial<Record<string, unknown>> = {}
): PayloadSection[] {
  return [
    {
      id: 'input',
      label: 'Input',
      value:
        'input' in overrides
          ? overrides.input
          : { data: { catalogId: 'cat_eu' }, variables: {} },
      emptyState: {
        title: 'No input data',
        description: 'This run started without input.',
      },
    },
    {
      id: 'output',
      label: 'Output',
      value:
        'output' in overrides
          ? overrides.output
          : {
              feeds_seen: 14,
              enriched: [
                { sku: 'ACM-10432', seo_score: 82 },
                { sku: 'NRD-2291', seo_score: 74 },
              ],
            },
    },
    {
      id: 'state',
      label: 'State',
      value:
        'state' in overrides ? overrides.state : { phase: 'done', counter: 3 },
      meta: 'Updated 7 hr ago',
      fieldLabels: {
        counter: { label: 'Items counted', description: 'How many items' },
      },
    },
  ];
}

const rowById = (id: string) =>
  document.querySelector<HTMLElement>(
    `[role="treeitem"][data-row-id="${CSS.escape(id)}"]`
  );
const treeItems = () => screen.queryAllByRole('treeitem');
const tree = () => screen.getByRole('tree');
const inspector = () => screen.getByRole('group', { name: 'Selected field' });

function setup(
  props: Partial<React.ComponentProps<typeof PayloadExplorer>> = {}
) {
  const user = userEvent.setup();
  const writeText = vi.spyOn(navigator.clipboard, 'writeText');
  const view = render(
    <PayloadExplorer
      sections={runSections()}
      defaultSectionId="output"
      {...props}
    />
  );
  return { user, writeText, ...view };
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe('PayloadExplorer', () => {
  it('opens on the default section with each section’s top-level key count', () => {
    setup();
    const tabs = screen.getAllByRole('tab');
    expect(tabs.map((t) => t.textContent)).toEqual([
      'Input2',
      'Output2',
      'State2',
    ]);
    expect(screen.getByRole('tab', { name: /Output/ })).toHaveAttribute(
      'aria-selected',
      'true'
    );
    // The small top-level branch opens by itself.
    expect(rowById('enriched[0]')).toBeInTheDocument();
    expect(rowById('enriched')).toHaveAttribute('aria-expanded', 'true');
  });

  it('shows a section’s empty state, meta and field labels', async () => {
    const { user } = setup({ sections: runSections({ input: null }) });
    await user.click(screen.getByRole('tab', { name: /Input/ }));
    expect(screen.getByText('No input data')).toBeInTheDocument();
    expect(
      screen.getByText('This run started without input.')
    ).toBeInTheDocument();
    expect(screen.queryByRole('tree')).not.toBeInTheDocument();

    await user.click(screen.getByRole('tab', { name: /State/ }));
    expect(screen.getByText('Updated 7 hr ago')).toBeInTheDocument();
    expect(
      within(rowById('counter')!).getByText('Items counted')
    ).toHaveAttribute('title', 'How many items');
  });

  it('switches sections with arrow keys on the tab list', async () => {
    const { user } = setup();
    screen.getByRole('tab', { name: /Output/ }).focus();
    await user.keyboard('{ArrowRight}');
    expect(screen.getByRole('tab', { name: /State/ })).toHaveAttribute(
      'aria-selected',
      'true'
    );
    expect(screen.getByRole('tab', { name: /State/ })).toHaveFocus();
    await user.keyboard('{ArrowRight}');
    expect(screen.getByRole('tab', { name: /Input/ })).toHaveAttribute(
      'aria-selected',
      'true'
    );
  });

  it('folds and unfolds branches with the chevron', async () => {
    const { user } = setup();
    await user.click(
      within(rowById('enriched')!).getByRole('button', { name: 'Collapse' })
    );
    expect(rowById('enriched[0]')).not.toBeInTheDocument();
    expect(rowById('enriched')).toHaveTextContent('2 items');
    await user.click(
      within(rowById('enriched')!).getByRole('button', { name: 'Expand' })
    );
    expect(rowById('enriched[0]')).toBeInTheDocument();
  });

  it('navigates and folds with the keyboard', async () => {
    const { user } = setup();
    await user.click(rowById('feeds_seen')!);
    tree().focus();
    await user.keyboard('{ArrowDown}');
    expect(rowById('enriched')).toHaveAttribute('aria-selected', 'true');
    expect(tree()).toHaveAttribute(
      'aria-activedescendant',
      rowById('enriched')!.id
    );
    await user.keyboard('{ArrowLeft}');
    expect(rowById('enriched')).toHaveAttribute('aria-expanded', 'false');
    await user.keyboard('{ArrowRight}{ArrowRight}');
    expect(rowById('enriched[0]')).toHaveAttribute('aria-selected', 'true');
    await user.keyboard('{ArrowRight}{ArrowDown}');
    expect(rowById('enriched[0].sku')).toHaveAttribute('aria-selected', 'true');
    await user.keyboard('{ArrowLeft}');
    expect(rowById('enriched[0]')).toHaveAttribute('aria-selected', 'true');
    await user.keyboard('{End}');
    expect(rowById('enriched[1]')).toHaveAttribute('aria-selected', 'true');
    await user.keyboard('{Home}');
    expect(rowById('feeds_seen')).toHaveAttribute('aria-selected', 'true');
  });

  it('searches every section, reveals matches and steps through them', async () => {
    const { user } = setup({
      sections: runSections({
        output: {
          enriched: [{ meta: { sku: 'A-1' } }, { meta: { sku: 'B-2' } }],
        },
      }),
    });
    await user.click(rowById('enriched[0]')!.querySelector('button')!); // fold
    await user.type(
      screen.getByRole('searchbox', { name: 'Search keys and values' }),
      'sku'
    );

    await waitFor(() => expect(screen.getByText('1 of 2')).toBeInTheDocument());
    expect(screen.getByRole('tab', { name: /Output/ })).toHaveTextContent(
      'Output2'
    );
    expect(screen.getByRole('tab', { name: /Input/ })).toHaveTextContent(
      'Input0'
    );
    // Branches holding matches open, and the first match is selected.
    expect(rowById('enriched[0].meta.sku')).toHaveAttribute(
      'aria-selected',
      'true'
    );
    expect(
      rowById('enriched[0].meta.sku')!.querySelector('mark')
    ).toHaveTextContent('sku');

    await user.keyboard('{Enter}');
    expect(screen.getByText('2 of 2')).toBeInTheDocument();
    expect(rowById('enriched[1].meta.sku')).toHaveAttribute(
      'aria-selected',
      'true'
    );
    await user.keyboard('{Shift>}{Enter}{/Shift}');
    expect(screen.getByText('1 of 2')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Next match' }));
    expect(screen.getByText('2 of 2')).toBeInTheDocument();

    await user.click(screen.getByRole('searchbox'));
    await user.keyboard('{Escape}');
    expect(screen.queryByText(/of 2/)).not.toBeInTheDocument();
    expect(screen.getByRole('tab', { name: /Output/ })).toHaveTextContent(
      'Output1'
    );
  });

  it('moves to a section that has matches when the current one has none', async () => {
    const { user } = setup();
    await user.type(screen.getByRole('searchbox'), 'catalog');
    await waitFor(() =>
      expect(screen.getByRole('tab', { name: /Input/ })).toHaveAttribute(
        'aria-selected',
        'true'
      )
    );
    expect(rowById('data.catalogId')).toHaveAttribute('aria-selected', 'true');
  });

  it('keeps only matching branches when filtering', async () => {
    const { user } = setup();
    await user.type(screen.getByRole('searchbox'), 'NRD');
    await waitFor(() => expect(screen.getByText('1 of 1')).toBeInTheDocument());
    expect(rowById('feeds_seen')).toBeInTheDocument();
    await user.click(screen.getByRole('checkbox', { name: 'Only matches' }));
    expect(rowById('feeds_seen')).not.toBeInTheDocument();
    expect(treeItems().map((r) => r.dataset.rowId)).toEqual([
      'enriched',
      'enriched[1]',
      'enriched[1].sku',
    ]);
  });

  it('explains when filtering leaves a section empty', async () => {
    const { user } = setup();
    await user.click(screen.getByRole('checkbox', { name: 'Only matches' }));
    await user.type(screen.getByRole('searchbox'), 'phase');
    await waitFor(() =>
      expect(screen.getByRole('tab', { name: /State/ })).toHaveAttribute(
        'aria-selected',
        'true'
      )
    );
    await user.click(screen.getByRole('tab', { name: /Output/ }));
    expect(screen.getByText('No matches in output')).toBeInTheDocument();
    expect(screen.getByText('Found 1 in state.')).toBeInTheDocument();
  });

  it('copies a field’s path and value from the inspector and the keyboard', async () => {
    const { user, writeText } = setup();
    await user.click(rowById('enriched[0]')!);
    expect(
      screen.getByRole('navigation', { name: 'Selected path' })
    ).toHaveTextContent('Output›enriched›[0]');
    expect(screen.getByText('object · 2 keys')).toBeInTheDocument();

    await user.click(
      within(inspector()).getByRole('button', { name: 'Copy path' })
    );
    expect(writeText).toHaveBeenLastCalledWith('enriched[0]');
    await user.click(
      within(inspector()).getByRole('button', { name: 'Copy value' })
    );
    expect(writeText).toHaveBeenLastCalledWith(
      JSON.stringify({ sku: 'ACM-10432', seo_score: 82 }, null, 2)
    );
    expect(toast.success).toHaveBeenCalledWith('Copied value');

    await user.click(rowById('feeds_seen')!);
    tree().focus();
    await user.keyboard('v');
    expect(writeText).toHaveBeenLastCalledWith('14');
    await user.keyboard('c');
    expect(writeText).toHaveBeenLastCalledWith('feeds_seen');
  });

  it('jumps to an ancestor from the path crumbs', async () => {
    const { user } = setup();
    await user.click(rowById('enriched[1]')!);
    await user.click(
      within(
        screen.getByRole('navigation', { name: 'Selected path' })
      ).getByRole('button', { name: 'enriched' })
    );
    expect(rowById('enriched')).toHaveAttribute('aria-selected', 'true');
  });

  it('copies a whole section as pretty JSON', async () => {
    const { user, writeText } = setup();
    await user.click(screen.getByRole('button', { name: 'Copy JSON' }));
    expect(writeText).toHaveBeenCalledWith(
      JSON.stringify(runSections()[1].value, null, 2)
    );
  });

  it('reports a blocked clipboard instead of failing silently', async () => {
    const { user, writeText } = setup();
    writeText.mockRejectedValueOnce(new Error('denied'));
    await user.click(screen.getByRole('button', { name: 'Copy JSON' }));
    expect(toast.error).toHaveBeenCalledWith(
      'Could not copy: the browser blocked clipboard access.'
    );
  });

  it('shows the JSON text view with line numbers', async () => {
    const { user } = setup();
    await user.click(screen.getByRole('button', { name: 'JSON' }));
    const region = screen.getByRole('region', { name: 'Output as JSON' });
    expect(region).toHaveTextContent('"feeds_seen": 14');
    expect(region).toHaveTextContent(/^1\{/);
    expect(screen.getByRole('button', { name: 'Expand all' })).toBeDisabled();
  });

  describe('compact', () => {
    it('fits one toolbar row with icon actions and shows the path strip only after selecting', async () => {
      const { user, writeText } = setup({ compact: true });
      expect(screen.getByRole('searchbox')).toHaveAttribute(
        'placeholder',
        'Search'
      );
      expect(
        screen.queryByRole('checkbox', { name: 'Only matches' })
      ).toBeNull();
      expect(
        screen.getByRole('button', { name: 'Only matches' })
      ).toHaveAttribute('aria-pressed', 'false');
      expect(
        screen.queryByRole('group', { name: 'Selected field' })
      ).toBeNull();

      await user.click(rowById('feeds_seen')!);
      expect(inspector()).toHaveTextContent('Output›feeds_seen');
      await user.click(
        within(inspector()).getByRole('button', { name: 'Copy path' })
      );
      expect(writeText).toHaveBeenLastCalledWith('feeds_seen');

      await user.type(screen.getByRole('searchbox'), 'NRD');
      await waitFor(() =>
        expect(screen.getByText('1 of 1')).toBeInTheDocument()
      );
      await user.click(screen.getByRole('button', { name: 'Only matches' }));
      expect(
        screen.getByRole('button', { name: 'Only matches' })
      ).toHaveAttribute('aria-pressed', 'true');
      expect(rowById('feeds_seen')).toBeNull();

      await user.click(screen.getByRole('button', { name: 'Copy JSON' }));
      expect(writeText).toHaveBeenLastCalledWith(
        JSON.stringify(runSections()[1].value, null, 2)
      );
    });

    it('shows a single section as a label instead of tabs', () => {
      render(
        <PayloadExplorer
          compact
          sections={[{ id: 'only', label: 'Outputs', value: { a: 1 } }]}
        />
      );
      expect(screen.queryByRole('tablist')).toBeNull();
      expect(
        screen.getByRole('tabpanel', { name: 'Outputs' })
      ).toBeInTheDocument();
    });
  });

  describe('server-truncated values', () => {
    const truncated = {
      _truncated: true,
      _original_size: 1_258_291,
      _preview: '{"items":[{"sku":"ACM-1"},{"sku":"ACM-2"',
    };

    it('marks the value as cut off, shows its stored part and copies only that', async () => {
      const { user, writeText } = setup({
        sections: runSections({ output: { payload: truncated } }),
      });
      expect(rowById('payload')).toHaveTextContent('Cut off · 1.2 MB');
      expect(rowById('payload')).toHaveTextContent('{"items":[{"sku":"ACM-1"}');
      expect(
        screen.getByText(
          /1 value cut off by the server \(1\.2 MB originally\); only the stored part is shown/
        )
      ).toBeInTheDocument();

      await user.click(rowById('payload')!);
      expect(
        screen.getByText(/The server kept only the first 40 characters/)
      ).toBeInTheDocument();
      await user.click(
        within(inspector()).getByRole('button', { name: 'Copy value' })
      );
      expect(writeText).toHaveBeenLastCalledWith(truncated._preview);
      expect(toast.success).toHaveBeenLastCalledWith(
        'Copied the stored part of this value'
      );
    });

    it('treats a whole truncated payload as a single row', () => {
      setup({ sections: runSections({ output: truncated }) });
      expect(treeItems()).toHaveLength(1);
      expect(treeItems()[0]).toHaveTextContent('Cut off · 1.2 MB');
    });
  });

  describe('large values', () => {
    it('shows elided values by size and loads the full value on request', async () => {
      const loadFullValue = vi
        .fn()
        .mockResolvedValue({ report_pdf: 'JVBERi0xLjcK'.repeat(10) });
      const { user } = setup({
        sections: runSections({ output: { report_pdf: elided } }),
        loadFullValue,
      });
      expect(
        screen.getByText('1 large value not loaded (1.8 MB)')
      ).toBeInTheDocument();
      expect(rowById('report_pdf')).toHaveTextContent('Large value · 1.8 MB');

      await user.click(
        within(rowById('report_pdf')!).getByRole('button', {
          name: 'Load full value',
        })
      );
      expect(loadFullValue).toHaveBeenCalledWith('output');
      await waitFor(() =>
        expect(rowById('report_pdf')).toHaveTextContent('"JVBERi0xLjcK')
      );
      expect(toast.success).toHaveBeenCalledWith('Loaded the full output');
      expect(screen.queryByText(/not loaded/)).not.toBeInTheDocument();
    });

    it('loads the full value before copying a section that has elided parts', async () => {
      const full = { report_pdf: 'FULL-BASE64' };
      const loadFullValue = vi.fn().mockResolvedValue(full);
      const { user, writeText } = setup({
        sections: runSections({ output: { report_pdf: elided } }),
        loadFullValue,
      });
      await user.click(rowById('report_pdf')!);
      await user.click(
        within(inspector()).getByRole('button', { name: 'Copy value' })
      );
      await waitFor(() =>
        expect(writeText).toHaveBeenCalledWith('FULL-BASE64')
      );
      expect(loadFullValue).toHaveBeenCalledTimes(1);
    });

    it('reports a failed full-value load', async () => {
      const loadFullValue = vi.fn().mockRejectedValue(new Error('HTTP 500'));
      const { user } = setup({
        sections: runSections({ output: { report_pdf: elided } }),
        loadFullValue,
      });
      await user.click(
        within(rowById('report_pdf')!).getByRole('button', {
          name: 'Load full value',
        })
      );
      await waitFor(() =>
        expect(toast.error).toHaveBeenCalledWith(
          'Could not load the full output: HTTP 500'
        )
      );
      expect(rowById('report_pdf')).toHaveTextContent('Large value · 1.8 MB');
    });

    it('offers no load button without a loader', () => {
      setup({ sections: runSections({ output: { report_pdf: elided } }) });
      expect(
        within(rowById('report_pdf')!).queryByRole('button', {
          name: 'Load full value',
        })
      ).toBeNull();
    });

    it('truncates long strings in rows and previews them in the inspector', async () => {
      const long = 'x'.repeat(50_000);
      const { user } = setup({
        sections: runSections({ output: { blob: long } }),
      });
      const row = rowById('blob')!;
      expect(row.textContent!.length).toBeLessThan(400);
      expect(row).toHaveTextContent('50,000 chars');
      await user.click(row);
      expect(screen.getByLabelText('Value preview').textContent).toHaveLength(
        10_000
      );
      expect(
        screen.getByText(/Showing the first 10,000 of 50,000 characters/)
      ).toBeInTheDocument();
    });

    it('groups wide arrays and renders only the rows in view', async () => {
      const wide = {
        rows: Array.from({ length: 5_000 }, (_, i) => ({
          id: i,
          name: `row ${i}`,
        })),
      };
      const { user } = setup({
        sections: runSections({ output: wide }),
        maxHeight: 280,
      });
      await user.click(
        within(rowById('rows')!).getByRole('button', { name: 'Expand' })
      );
      expect(rowById('rows#0-99')).toHaveTextContent('[0 … 99]100 items');
      expect(rowById('rows#4900-4999')).toBeNull(); // 51st row: outside the window

      await user.click(screen.getByRole('button', { name: 'Expand all' }));
      // Thousands of rows exist, but only the window is in the DOM.
      const spacer = tree().firstElementChild as HTMLElement;
      expect(parseInt(spacer.style.height, 10)).toBeGreaterThan(28 * 1_900);
      expect(treeItems().length).toBeLessThanOrEqual(10 + 16 + 1);
      expect(toast.info).toHaveBeenCalledWith(
        'Opened the first 2,000 branches. Search to reach the rest.'
      );
    });

    it('handles a payload over 1 MB: search reveals and scrolls to a deep match', async () => {
      const items = 10_000;
      const big = {
        enriched: Array.from({ length: items }, (_, i) => ({
          sku: `SKU-${String(i).padStart(6, '0')}`,
          title: `Industrial part ${i} with a reasonably long product description`,
          market_price: { min: 1.5, median: 2.25, max: 3.75, currency: 'EUR' },
        })),
      };
      expect(JSON.stringify(big).length).toBeGreaterThan(1_000_000);
      const { user } = setup({ sections: runSections({ output: big }) });
      expect(screen.getByTitle('Approximate JSON size')).toHaveTextContent(
        /~1\.\d MB/
      );

      await user.type(screen.getByRole('searchbox'), 'SKU-009876');
      await waitFor(() =>
        expect(rowById('enriched[9876].sku')).toHaveAttribute(
          'aria-selected',
          'true'
        )
      );
      expect(screen.getByText('1 of 1')).toBeInTheDocument();

      expect(tree().scrollTop).toBeGreaterThan(0);
      expect(treeItems().length).toBeLessThan(60);
    });

    it('declines the JSON view for payloads over the text limit', async () => {
      const huge = { blob: 'y'.repeat(2_200_000) };
      const { user } = setup({ sections: runSections({ output: huge }) });
      await user.click(screen.getByRole('button', { name: 'JSON' }));
      expect(
        screen.getByText('This output is too large to show as text')
      ).toBeInTheDocument();
      expect(
        screen.queryByRole('region', { name: 'Output as JSON' })
      ).toBeNull();
    });
  });
});
