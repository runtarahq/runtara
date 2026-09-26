import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ChatInput } from './index';
vi.mock('./ChatFormInput', () => ({
  ChatFormInput: () => <div>Structured response</div>,
}));
afterEach(cleanup);
const input = { requestId: 'request', signalId: 'signal', message: 'Question' };
const props = () => ({
  status: 'waiting_for_input' as const,
  waitingForInput: input,
  pendingInputs: [input],
  onSend: vi.fn().mockResolvedValue(false),
  onSelectInput: vi.fn(),
  onSubmitInput: vi.fn(),
});
describe('chat response drafts', () => {
  it('retains text on uncertain failure and clears it only after confirmation', async () => {
    const options = props();
    const { rerender } = render(<ChatInput {...options} />);
    fireEvent.change(screen.getByRole('textbox'), {
      target: { value: 'My answer' },
    });
    fireEvent.keyDown(screen.getByRole('textbox'), { key: 'Enter' });
    await waitFor(() =>
      expect(options.onSend).toHaveBeenCalledWith('My answer')
    );
    expect(screen.getByRole('textbox')).toHaveValue('My answer');
    options.onSend.mockResolvedValue(true);
    rerender(<ChatInput {...options} />);
    fireEvent.keyDown(screen.getByRole('textbox'), { key: 'Enter' });
    await waitFor(() => expect(screen.getByRole('textbox')).toHaveValue(''));
  });
  it('never carries a stale response into the next target', () => {
    const options = props();
    const { rerender } = render(<ChatInput {...options} />);
    fireEvent.change(screen.getByRole('textbox'), {
      target: { value: 'Old answer' },
    });
    rerender(
      <ChatInput
        {...options}
        waitingForInput={{ ...input, requestId: 'next' }}
      />
    );
    expect(screen.getByRole('textbox')).toHaveValue('');
    expect(options.onSend).not.toHaveBeenCalled();
  });
  it('requires an explicit selection for ambiguous requests', () => {
    const options = props();
    render(
      <ChatInput
        {...options}
        waitingForInput={null}
        pendingInputs={[
          input,
          { ...input, requestId: 'next', message: 'Second question' },
        ]}
      />
    );
    expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Second question' }));
    expect(options.onSelectInput).toHaveBeenCalledWith(
      expect.objectContaining({ requestId: 'next' })
    );
    expect(options.onSend).not.toHaveBeenCalled();
  });
});
