import { Input } from '@/shared/components/ui/input';
import { Label } from '@/shared/components/ui/label';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select';
import { useCustomQuery } from '@/shared/hooks/api';
import { queryKeys } from '@/shared/queries/query-keys';
import { getWorkflows } from '@/features/workflows/queries';
import { WorkflowDto } from '@/generated/RuntaraRuntimeApi';
import {
  ReportBlockDefinition,
  ReportFileUploadConfig,
  ReportFileUploadTrigger,
  ReportWorkflowActionConfig,
} from '../../../types';

const MAX_SIZE_MB = 50;

interface FileUploadBlockEditorProps {
  block: ReportBlockDefinition;
  onChange: (block: ReportBlockDefinition) => void;
}

type WorkflowOption = { id: string; name: string };

export function FileUploadBlockEditor({
  block,
  onChange,
}: FileUploadBlockEditorProps) {
  const config: ReportFileUploadConfig = block.file_upload ?? {
    workflowAction: {
      id: 'upload',
      workflowId: '',
      context: { mode: 'value', inputKey: 'file' },
    },
  };
  const action = config.workflowAction;
  // Same narrowing trick as tableActionEditors' workflow picker.
  const workflows = useCustomQuery({
    queryKey: queryKeys.workflows.all,
    queryFn: getWorkflows,
    select: (response: any): WorkflowOption[] => {
      const content: WorkflowDto[] = response?.data?.content ?? [];
      return content.map((workflow) => ({
        id: workflow.id,
        name: workflow.name || workflow.id,
      }));
    },
  }) as unknown as { data?: WorkflowOption[]; isFetching: boolean };

  const update = (patch: Partial<ReportFileUploadConfig>) =>
    onChange({ ...block, file_upload: { ...config, ...patch } });
  const updateAction = (patch: Partial<ReportWorkflowActionConfig>) =>
    update({ workflowAction: { ...action, ...patch } });

  return (
    <div className="grid gap-3">
      <div className="grid gap-2 sm:grid-cols-2">
        <div className="grid gap-1.5">
          <Label className="text-xs">Workflow</Label>
          <Select
            value={action.workflowId || ''}
            onValueChange={(workflowId) => updateAction({ workflowId })}
            disabled={workflows.isFetching}
          >
            <SelectTrigger data-testid={`file-upload-workflow-${block.id}`}>
              <SelectValue
                placeholder={
                  workflows.isFetching ? 'Loading…' : 'Select workflow'
                }
              />
            </SelectTrigger>
            <SelectContent>
              {(workflows.data ?? []).map((workflow) => (
                <SelectItem key={workflow.id} value={workflow.id}>
                  {workflow.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        <div className="grid gap-1.5">
          <Label className="text-xs">Trigger</Label>
          <Select
            value={config.trigger ?? 'button'}
            onValueChange={(trigger) =>
              update({ trigger: trigger as ReportFileUploadTrigger })
            }
          >
            <SelectTrigger data-testid={`file-upload-trigger-${block.id}`}>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="button">
                Button — run when the button is pressed
              </SelectItem>
              <SelectItem value="automatic">
                Automatic — run as soon as a file is chosen
              </SelectItem>
            </SelectContent>
          </Select>
        </div>
      </div>

      <div className="grid gap-2 sm:grid-cols-2">
        <div className="grid gap-1.5">
          <Label className="text-xs">Drop-zone title</Label>
          <Input
            value={config.title ?? ''}
            placeholder="Click to upload or drag and drop"
            onChange={(event) =>
              update({ title: event.target.value || undefined })
            }
          />
        </div>
        <div className="grid gap-1.5">
          <Label className="text-xs">Helper text</Label>
          <Input
            value={config.description ?? ''}
            placeholder="Shown inside the drop zone"
            onChange={(event) =>
              update({ description: event.target.value || undefined })
            }
          />
        </div>
      </div>

      <div className="grid gap-2 sm:grid-cols-3">
        <div className="grid gap-1.5">
          <Label className="text-xs">Button label</Label>
          <Input
            value={action.label ?? ''}
            placeholder="Run workflow"
            disabled={config.trigger === 'automatic'}
            onChange={(event) =>
              updateAction({ label: event.target.value || undefined })
            }
          />
        </div>
        <div className="grid gap-1.5">
          <Label className="text-xs">Running label</Label>
          <Input
            value={action.runningLabel ?? ''}
            placeholder="Running…"
            onChange={(event) =>
              updateAction({ runningLabel: event.target.value || undefined })
            }
          />
        </div>
        <div className="grid gap-1.5">
          <Label className="text-xs">Success message</Label>
          <Input
            value={action.successMessage ?? ''}
            placeholder="Workflow completed"
            onChange={(event) =>
              updateAction({ successMessage: event.target.value || undefined })
            }
          />
        </div>
      </div>

      <div className="grid gap-2 sm:grid-cols-3">
        <div className="grid gap-1.5">
          <Label className="text-xs">Workflow input key</Label>
          <Input
            value={action.context?.inputKey ?? ''}
            placeholder="file"
            onChange={(event) =>
              updateAction({
                context: {
                  ...action.context,
                  mode: 'value',
                  inputKey: event.target.value || undefined,
                },
              })
            }
          />
        </div>
        <div className="grid gap-1.5">
          <Label className="text-xs">Accepted types</Label>
          <Input
            value={(config.accept ?? []).join(', ')}
            placeholder=".csv, text/csv"
            onChange={(event) =>
              update({
                accept: event.target.value
                  .split(',')
                  .map((part) => part.trim())
                  .filter(Boolean),
              })
            }
          />
        </div>
        <div className="grid gap-1.5">
          <Label className="text-xs">Max size (MB)</Label>
          <Input
            type="number"
            min={1}
            max={MAX_SIZE_MB}
            value={
              config.maxSizeBytes
                ? Math.round(config.maxSizeBytes / (1024 * 1024))
                : ''
            }
            placeholder={String(MAX_SIZE_MB)}
            onChange={(event) => {
              const mb = Number.parseInt(event.target.value, 10);
              update({
                maxSizeBytes:
                  Number.isFinite(mb) && mb > 0
                    ? Math.min(mb, MAX_SIZE_MB) * 1024 * 1024
                    : undefined,
              });
            }}
          />
        </div>
      </div>

      <p className="text-xs text-muted-foreground">
        The selected file is sent to the workflow as{' '}
        <code>{'{content, filename, mimeType}'}</code> under the input key —
        declare a matching input-schema field of type <code>file</code> on the
        workflow.
      </p>
    </div>
  );
}
