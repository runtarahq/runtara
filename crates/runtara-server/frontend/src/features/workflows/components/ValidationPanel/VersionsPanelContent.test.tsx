import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';

import type { WorkflowVersionInfoDto } from '@/features/workflows/queries';
import { VersionsPanelContent } from './VersionsPanelContent';

function version(
  versionNumber: number,
  isActive: boolean,
  compiled = true
): WorkflowVersionInfoDto {
  return {
    versionId: `wf-v${versionNumber}`,
    versionNumber,
    workflowId: 'wf',
    isActive,
    compiled,
    trackEvents: false,
    createdAt: '2026-07-27T00:00:00Z',
    updatedAt: '2026-07-27T00:00:00Z',
  };
}

function renderPanel(versions: WorkflowVersionInfoDto[]) {
  render(
    <VersionsPanelContent
      versions={versions}
      onVersionChange={vi.fn()}
      onVersionActivate={vi.fn()}
    />
  );
}

/** The Active/Activate control sitting in a given version's row. */
function stateOf(versionNumber: number): string | undefined {
  const label = screen
    .getAllByText(`v${versionNumber}`)
    .find((node) => node.tagName === 'SPAN');
  const row = label?.closest('div[class*="border-b"]');
  return [...(row?.querySelectorAll('button') ?? [])]
    .map((button) => button.textContent?.trim())
    .find((text) => text === 'Active' || text === 'Activate');
}

/** The build control (Compile/Rebuild) in a given version's row, if rendered. */
function buildControlOf(versionNumber: number): HTMLButtonElement | undefined {
  const label = screen
    .getAllByText(`v${versionNumber}`)
    .find((node) => node.tagName === 'SPAN');
  const row = label?.closest('div[class*="border-b"]');
  return [...(row?.querySelectorAll('button') ?? [])].find((button) =>
    ['Compile', 'Rebuild', 'Rebuilding'].includes(
      button.textContent?.trim() ?? ''
    )
  ) as HTMLButtonElement | undefined;
}

describe('VersionsPanelContent', () => {
  it('marks the version the server reports as active', () => {
    // The server resolves the active version as current_version falling back
    // to latest_version, so a freshly saved version can be the one that
    // executes. Deriving the badge from a separately cached version number put
    // "Active" on the previous version while runs used the new one.
    renderPanel([version(1, false, false), version(2, true)]);

    expect(stateOf(2)).toBe('Active');
    expect(stateOf(1)).toBe('Activate');
  });

  it('follows the server when an older version is the active one', () => {
    renderPanel([version(1, true, false), version(2, false)]);

    expect(stateOf(1)).toBe('Active');
    expect(stateOf(2)).toBe('Activate');
  });

  it('does not offer Activate on the version that is already active', () => {
    renderPanel([version(1, false, false), version(2, true)]);

    const label = screen
      .getAllByText('v2')
      .find((node) => node.tagName === 'SPAN');
    const activeButton = [
      ...(label
        ?.closest('div[class*="border-b"]')
        ?.querySelectorAll('button') ?? []),
    ].find((button) => button.textContent?.trim() === 'Active');

    expect(activeButton).toBeDefined();
    expect((activeButton as HTMLButtonElement).disabled).toBe(true);
  });

  // A row with no compilation record at all — never built, or a rebuild that
  // invalidated the row and never landed — renders the amber "Not compiled"
  // badge. Gating the build control on compiled-or-failed left exactly that
  // row with no way to build it from the UI.
  it('offers a build control on a version that has never compiled', () => {
    const onVersionRebuild = vi.fn();
    render(
      <VersionsPanelContent
        versions={[version(43, true, false)]}
        onVersionChange={vi.fn()}
        onVersionActivate={vi.fn()}
        onVersionRebuild={onVersionRebuild}
      />
    );

    expect(screen.getByText('Not compiled')).toBeDefined();
    const build = buildControlOf(43);
    expect(build).toBeDefined();
    expect(build?.textContent?.trim()).toBe('Compile');
    expect(build?.disabled).toBe(false);

    fireEvent.click(build as HTMLButtonElement);
    expect(onVersionRebuild).toHaveBeenCalledWith(43);
  });

  it('still offers a rebuild on compiled and failed versions', () => {
    const failed: WorkflowVersionInfoDto = {
      ...version(2, false, false),
      compilationStatus: 'failed',
      errorMessage: 'boom',
    };
    render(
      <VersionsPanelContent
        versions={[version(1, true), failed]}
        onVersionChange={vi.fn()}
        onVersionActivate={vi.fn()}
        onVersionRebuild={vi.fn()}
      />
    );

    expect(buildControlOf(1)?.textContent?.trim()).toBe('Rebuild');
    expect(buildControlOf(2)?.textContent?.trim()).toBe('Rebuild');
  });
});
