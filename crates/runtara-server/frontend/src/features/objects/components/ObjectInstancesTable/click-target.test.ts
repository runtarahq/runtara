import { beforeEach, describe, expect, it } from 'vitest';

import { clickLandedInGrid } from './click-target';

/**
 * The console shell as the instances page renders it: a breadcrumb and toolbar
 * above the grid, a footer below, all inside one container.
 */
function buildShell() {
  document.body.innerHTML = `
    <div data-shell>
      <nav aria-label="Breadcrumb"><span id="crumb">Objects</span></nav>
      <div id="toolbar"><button id="export">Export</button></div>
      <table>
        <tbody>
          <tr><td id="cell">before</td></tr>
        </tbody>
      </table>
      <div id="footer"><button id="next">Next</button></div>
    </div>
  `;
  return document.querySelector('[data-shell]') as HTMLElement;
}

describe('clickLandedInGrid', () => {
  let shell: HTMLElement;

  beforeEach(() => {
    shell = buildShell();
  });

  it('counts a click on a cell as inside the grid', () => {
    expect(clickLandedInGrid(document.querySelector('#cell'), shell)).toBe(
      true
    );
  });

  it('counts the breadcrumb as outside, so a pending edit gets flushed', () => {
    // This is the regression: the check used to match the whole shell, so the
    // breadcrumb read as "inside the table" and the edit was never written.
    expect(clickLandedInGrid(document.querySelector('#crumb'), shell)).toBe(
      false
    );
  });

  it('counts the toolbar and footer as outside', () => {
    expect(clickLandedInGrid(document.querySelector('#export'), shell)).toBe(
      false
    );
    expect(clickLandedInGrid(document.querySelector('#next'), shell)).toBe(
      false
    );
  });

  it('counts anything beyond the shell as outside', () => {
    const sidebar = document.createElement('a');
    document.body.appendChild(sidebar);
    expect(clickLandedInGrid(sidebar, shell)).toBe(false);
  });

  it('is safe before the shell has mounted', () => {
    expect(clickLandedInGrid(document.querySelector('#cell'), null)).toBe(
      false
    );
    expect(clickLandedInGrid(null, shell)).toBe(false);
  });

  it('is safe when the shell holds no grid yet', () => {
    document.body.innerHTML = `<div data-shell><p>Loading…</p></div>`;
    const empty = document.querySelector('[data-shell]') as HTMLElement;
    expect(clickLandedInGrid(empty.querySelector('p'), empty)).toBe(false);
  });
});
