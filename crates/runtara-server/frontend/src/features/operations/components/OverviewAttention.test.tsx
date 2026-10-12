import { render, screen, within } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { MemoryRouter } from 'react-router';
import { OverviewAttention } from './OverviewAttention';

const props = {
  requests: [],
  requestCount: 0,
  requestsPending: false,
  requestsError: false,
};

describe('Overview attention', () => {
  it('shows a failed prerequisite as an error instead of loading or an empty queue', () => {
    render(
      <MemoryRouter>
        <OverviewAttention {...props} requestsPending requestsError />
      </MemoryRouter>
    );
    expect(
      within(
        screen.getByRole('region', { name: 'Requests requiring input' })
      ).getByText('Could not load this group. Try Refresh.')
    ).toBeInTheDocument();
    expect(
      screen.getByRole('link', { name: 'View all requests' })
    ).toHaveAttribute('href', '/operations/requests');
  });
});
