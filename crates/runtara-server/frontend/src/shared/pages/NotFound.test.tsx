import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router';

import { NotFound } from './NotFound';

// Mounted through a wildcard route rather than rendered directly, so the test
// exercises the same shape as the router: an unmatched URL falls through to
// this page and the page reads the attempted path off the location.
function renderAt(path: string) {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <Routes>
        <Route path="/workflows" element={<div>WORKFLOWS</div>} />
        <Route path="*" element={<NotFound />} />
      </Routes>
    </MemoryRouter>
  );
}

describe('NotFound', () => {
  it('renders an explained not-found screen, not a bare 404', () => {
    renderAt('/no-such-page');

    expect(
      screen.getByRole('heading', { name: /page not found/i })
    ).toBeInTheDocument();
    expect(screen.getByText(/nothing at this address/i)).toBeInTheDocument();
    // The old wildcard element rendered the literal string "404" and nothing
    // else — that is exactly what this page replaces.
    expect(screen.queryByText('404')).not.toBeInTheDocument();
  });

  it('echoes the attempted path so a typo is visible', () => {
    renderAt('/workflowz');

    expect(screen.getByText('/workflowz')).toBeInTheDocument();
  });

  it('offers a link back to workflows', () => {
    renderAt('/no-such-page');

    expect(
      screen.getByRole('link', { name: /back to workflows/i })
    ).toHaveAttribute('href', '/workflows');
  });

  it('offers a go-back control alongside the link', () => {
    renderAt('/no-such-page');

    expect(
      screen.getByRole('button', { name: /go back/i })
    ).toBeInTheDocument();
  });
});
