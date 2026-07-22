import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { FileUploadBlock } from './FileUploadBlock';
import type { ReportBlockDefinition } from '../../types';

vi.mock('react-oidc-context', () => ({
  useAuth: () => ({ user: { access_token: 'test-token' } }),
}));

vi.mock('sonner', () => ({
  toast: { success: vi.fn(), error: vi.fn() },
}));

const executeReportWorkflowAction = vi.fn();
const getReportWorkflowInstanceStatus = vi.fn();
const runReportWorkflow = vi.fn();

vi.mock('../../queries', () => ({
  executeReportWorkflowAction: (...args: unknown[]) =>
    executeReportWorkflowAction(...args),
  getReportWorkflowInstanceStatus: (...args: unknown[]) =>
    getReportWorkflowInstanceStatus(...args),
  runReportWorkflow: (...args: unknown[]) => runReportWorkflow(...args),
}));

function uploadBlock(
  overrides: Record<string, unknown> = {}
): ReportBlockDefinition {
  return {
    id: 'csv_import',
    type: 'file_upload',
    source: { schema: '' },
    file_upload: {
      title: 'Import price list',
      description: 'Drop a CSV here.',
      accept: ['.csv'],
      trigger: 'button',
      workflowAction: {
        id: 'upload',
        workflowId: 'import_prices',
        label: 'Import',
        runningLabel: 'Importing…',
        context: { mode: 'value', inputKey: 'file' },
      },
      ...overrides,
    },
  } as ReportBlockDefinition;
}

function completedExecution() {
  return {
    completedWithinWait: true,
    execution: {
      workflowId: 'import_prices',
      instanceId: 'inst-1',
      status: 'completed',
      output: null,
      error: null,
      version: 1,
      durationMs: 40,
    },
    render: undefined,
    canonicalViewId: null,
  };
}

function renderBlock(
  block = uploadBlock(),
  onRefresh: () => void = () => undefined
) {
  return render(
    <FileUploadBlock
      reportId="rep_abc"
      activeViewId={null}
      block={block}
      filters={{}}
      onRefresh={onRefresh}
    />
  );
}

function selectFile(blockId: string, file: File) {
  const input = screen.getByTestId(`file-upload-input-${blockId}`);
  fireEvent.change(input, { target: { files: [file] } });
}

beforeEach(() => {
  executeReportWorkflowAction.mockReset();
  getReportWorkflowInstanceStatus.mockReset();
  runReportWorkflow.mockReset();
});

describe('FileUploadBlock (button mode)', () => {
  it('shows the selected file and only runs when the button is pressed', async () => {
    executeReportWorkflowAction.mockResolvedValue(completedExecution());
    renderBlock();

    expect(screen.getByText('Import price list')).not.toBeNull();
    selectFile('csv_import', new File(['a,b\n1,2'], 'prices.csv'));

    await waitFor(() => expect(screen.queryByText('prices.csv')).not.toBeNull());
    expect(executeReportWorkflowAction).not.toHaveBeenCalled();

    fireEvent.click(screen.getByTestId('file-upload-run-csv_import'));

    await waitFor(() =>
      expect(executeReportWorkflowAction).toHaveBeenCalledTimes(1)
    );
    const [, request] = executeReportWorkflowAction.mock.calls[0] as [
      string,
      {
        reportId: string;
        blockId: string;
        actionId: string;
        idempotencyKey: string;
        body: { trigger: { value: unknown } };
      },
    ];
    expect(request.reportId).toBe('rep_abc');
    expect(request.blockId).toBe('csv_import');
    expect(request.actionId).toBe('upload');
    expect(request.idempotencyKey).toBeTruthy();
    expect(request.body.trigger.value).toMatchObject({
      // 'a,b\n1,2' base64-encoded — the canonical FileData wire shape.
      content: btoa('a,b\n1,2'),
      filename: 'prices.csv',
    });

    // The chip clears once the run completes.
    await waitFor(() => expect(screen.queryByText('prices.csv')).toBeNull());
  });

  it('invokes onRefresh after a completed run', async () => {
    executeReportWorkflowAction.mockResolvedValue(completedExecution());
    const onRefresh = vi.fn();
    renderBlock(uploadBlock(), onRefresh);

    selectFile('csv_import', new File(['x'], 'prices.csv'));
    await waitFor(() => expect(screen.queryByText('prices.csv')).not.toBeNull());
    fireEvent.click(screen.getByTestId('file-upload-run-csv_import'));

    await waitFor(() => expect(onRefresh).toHaveBeenCalledTimes(1));
  });
});

describe('FileUploadBlock (automatic mode)', () => {
  it('runs the workflow as soon as a file is selected', async () => {
    executeReportWorkflowAction.mockResolvedValue(completedExecution());
    renderBlock(uploadBlock({ trigger: 'automatic' }));

    selectFile('csv_import', new File(['x'], 'auto.csv'));

    await waitFor(() =>
      expect(executeReportWorkflowAction).toHaveBeenCalledTimes(1)
    );
    // No run button in automatic mode.
    expect(screen.queryByTestId('file-upload-run-csv_import')).toBeNull();
  });
});

describe('FileUploadBlock validation', () => {
  it('rejects files over maxSizeBytes without calling the API', async () => {
    renderBlock(uploadBlock({ maxSizeBytes: 4 }));

    selectFile('csv_import', new File(['12345'], 'big.csv'));

    await waitFor(() =>
      expect(screen.queryByText(/exceeds maximum allowed/)).not.toBeNull()
    );
    expect(executeReportWorkflowAction).not.toHaveBeenCalled();
  });

  it('rejects dropped files that do not match accept', async () => {
    renderBlock();

    const dropzone = screen.getByTestId('file-upload-dropzone-csv_import');
    fireEvent.drop(dropzone, {
      dataTransfer: { files: [new File(['x'], 'notes.txt')] },
    });

    await waitFor(() =>
      expect(screen.queryByText(/File type not accepted/)).not.toBeNull()
    );
    expect(executeReportWorkflowAction).not.toHaveBeenCalled();
  });
});
