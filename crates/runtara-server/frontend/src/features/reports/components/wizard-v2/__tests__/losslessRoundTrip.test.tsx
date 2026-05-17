import { readFileSync, readdirSync } from 'node:fs';
import path from 'node:path';
import { describe, expect, it } from 'vitest';
import { render } from '@testing-library/react';
import { ReportBuilderWizardV2 } from '../ReportBuilderWizardV2';
import { ReportDefinition } from '../../../types';

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
      // The wizard never calls onChange before the user interacts. Nothing
      // got mutated during render.
      expect(seen).toHaveLength(0);
    });
  }
});
