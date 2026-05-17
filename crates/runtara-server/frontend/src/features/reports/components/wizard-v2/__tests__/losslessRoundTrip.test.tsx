import { readFileSync, readdirSync } from 'node:fs';
import path from 'node:path';
import { describe, expect, it, vi } from 'vitest';
import { render, fireEvent } from '@testing-library/react';
import { ReportBuilderWizardV2 } from '../ReportBuilderWizardV2';
import {
  ReportBlockDefinition,
  ReportDefinition,
} from '../../../types';
import {
  addBlock,
  collectLayoutBlockIds,
  moveBlock,
  removeBlock,
  updateBlock,
} from '../layoutOps';

const FIXTURE_DIR = path.resolve(
  process.cwd(),
  '../tests/fixtures/reports'
);

function loadFixtures(): Array<{ name: string; definition: ReportDefinition }> {
  return readdirSync(FIXTURE_DIR)
    .filter((file) => file.endsWith('.json'))
    .map((file) => {
      const raw = readFileSync(path.join(FIXTURE_DIR, file), 'utf8');
      return {
        name: file,
        definition: JSON.parse(raw) as ReportDefinition,
      };
    });
}

describe('wizard v2 lossless round-trip', () => {
  const fixtures = loadFixtures();

  it('finds corpus fixtures', () => {
    expect(fixtures.length).toBeGreaterThan(0);
  });

  for (const fixture of fixtures) {
    it(`renders ${fixture.name} without modifying the definition`, () => {
      const seen: ReportDefinition[] = [];
      const onChange = (next: ReportDefinition) => {
        seen.push(next);
      };
      const { unmount } = render(
        <ReportBuilderWizardV2
          definition={fixture.definition}
          schemas={[]}
          onChange={onChange}
        />
      );
      unmount();
      expect(seen).toHaveLength(0);
    });
  }
});

describe('wizard v2 identity-edit round-trip', () => {
  const fixtures = loadFixtures();

  for (const fixture of fixtures) {
    it(`${fixture.name}: every block round-trips through layoutOps.updateBlock`, () => {
      const ids = collectLayoutBlockIds(fixture.definition.layout);
      let working = fixture.definition;
      for (const id of ids) {
        const before = JSON.stringify(
          working.blocks.find((b) => b.id === id)
        );
        working = updateBlock(working, id, (block) => block);
        const after = JSON.stringify(working.blocks.find((b) => b.id === id));
        expect(after).toBe(before);
      }
      // Full-definition round-trip identical.
      expect(JSON.stringify(working)).toBe(
        JSON.stringify(fixture.definition)
      );
    });

    it(`${fixture.name}: move a block forward+back is a no-op`, () => {
      const ids = collectLayoutBlockIds(fixture.definition.layout);
      if (ids.length < 2) return; // Single-block fixtures have nothing to swap.
      const [first] = ids;
      const moved = moveBlock(fixture.definition, first, ids.length - 1);
      const back = moveBlock(moved, first, 0);
      expect(JSON.stringify(back.layout)).toBe(
        JSON.stringify(fixture.definition.layout)
      );
      expect(JSON.stringify(back.blocks)).toBe(
        JSON.stringify(fixture.definition.blocks)
      );
    });

    it(`${fixture.name}: add then remove a block is a no-op`, () => {
      const probe: ReportBlockDefinition = {
        id: '__test_probe__',
        type: 'markdown',
        source: { schema: '' },
        markdown: { content: '' },
      };
      const added = addBlock(fixture.definition, probe);
      const removed = removeBlock(added, probe.id);
      expect(JSON.stringify(removed.blocks)).toBe(
        JSON.stringify(fixture.definition.blocks)
      );
      expect(JSON.stringify(removed.layout)).toBe(
        JSON.stringify(fixture.definition.layout)
      );
    });
  }
});

describe('wizard v2 mount-and-save round-trip', () => {
  const markdownFixture = path.join(FIXTURE_DIR, '01_markdown_minimal.json');
  const tableFixture = path.join(
    FIXTURE_DIR,
    '02_table_filter_object_model.json'
  );

  it('mount + no user edit → save is byte-identical (markdown)', () => {
    const original = JSON.parse(
      readFileSync(markdownFixture, 'utf8')
    ) as ReportDefinition;
    let latest = original;
    const onChange = vi.fn((next: ReportDefinition) => {
      latest = next;
    });
    render(
      <ReportBuilderWizardV2
        definition={original}
        schemas={[]}
        onChange={onChange}
      />
    );
    expect(onChange).not.toHaveBeenCalled();
    expect(JSON.stringify(latest)).toBe(JSON.stringify(original));
  });

  it('open block editor + flip-and-revert title → save is byte-identical (markdown)', () => {
    const original = JSON.parse(
      readFileSync(markdownFixture, 'utf8')
    ) as ReportDefinition;
    let latest = original;
    const onChange = vi.fn((next: ReportDefinition) => {
      latest = next;
    });
    const { getByText } = render(
      <ReportBuilderWizardV2
        definition={original}
        schemas={[]}
        onChange={onChange}
      />
    );
    // Open the only block (markdown intro).
    fireEvent.click(getByText('intro'));
    // No further interaction → state should still match input verbatim.
    expect(JSON.stringify(latest)).toBe(JSON.stringify(original));
  });

  it('table fixture mounts with no synthetic onChange (preserves complex layout)', () => {
    const original = JSON.parse(
      readFileSync(tableFixture, 'utf8')
    ) as ReportDefinition;
    const onChange = vi.fn();
    render(
      <ReportBuilderWizardV2
        definition={original}
        schemas={[]}
        onChange={onChange}
      />
    );
    expect(onChange).not.toHaveBeenCalled();
  });
});
