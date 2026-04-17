import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import { ConfirmationDialog } from './confirmation-dialog';

describe('ConfirmationDialog', () => {
  it('renders with default props', () => {
    render(
      <ConfirmationDialog open={true} onClose={() => {}} onConfirm={() => {}} />
    );

    // Check if default title and description are rendered
    expect(screen.getByText('Are you absolutely sure?')).toBeInTheDocument();
    expect(
      screen.getByText('This action will delete this entity.')
    ).toBeInTheDocument();

    // Check if buttons are rendered
    expect(screen.getByText('Cancel')).toBeInTheDocument();
    expect(screen.getByText('Confirm')).toBeInTheDocument();
  });

  it('renders with custom title and description', () => {
    render(
      <ConfirmationDialog
        open={true}
        title="Custom Title"
        description="Custom Description"
        onClose={() => {}}
        onConfirm={() => {}}
      />
    );

    expect(screen.getByText('Custom Title')).toBeInTheDocument();
    expect(screen.getByText('Custom Description')).toBeInTheDocument();
  });

  it('calls onClose when Cancel button is clicked', () => {
    const onClose = vi.fn();
    render(
      <ConfirmationDialog open={true} onClose={onClose} onConfirm={() => {}} />
    );

    fireEvent.click(screen.getByText('Cancel'));

    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('calls onConfirm when Confirm button is clicked', () => {
    const onConfirm = vi.fn();
    render(
      <ConfirmationDialog
        open={true}
        onClose={() => {}}
        onConfirm={onConfirm}
      />
    );

    fireEvent.click(screen.getByText('Confirm'));

    expect(onConfirm).toHaveBeenCalledTimes(1);
  });

  it('renders children when provided', () => {
    render(
      <ConfirmationDialog open={true} onClose={() => {}} onConfirm={() => {}}>
        <div data-testid="child-element">Child Content</div>
      </ConfirmationDialog>
    );

    expect(screen.getByTestId('child-element')).toBeInTheDocument();
    expect(screen.getByText('Child Content')).toBeInTheDocument();
  });

  it('shows loading state when loading prop is true', () => {
    render(
      <ConfirmationDialog
        open={true}
        loading={true}
        onClose={() => {}}
        onConfirm={() => {}}
      >
        <div data-testid="child-element">Child Content</div>
      </ConfirmationDialog>
    );

    expect(screen.getByText('Loading..')).toBeInTheDocument();

    // Children should not be rendered when loading
    expect(screen.queryByTestId('child-element')).not.toBeInTheDocument();
  });
});
