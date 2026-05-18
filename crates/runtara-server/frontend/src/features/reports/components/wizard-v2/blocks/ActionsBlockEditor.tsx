import { Input } from '@/shared/components/ui/input';
import { Label } from '@/shared/components/ui/label';
import { ReportBlockDefinition } from '../../../types';

interface ActionsBlockEditorProps {
  block: ReportBlockDefinition;
  onChange: (block: ReportBlockDefinition) => void;
}

export function ActionsBlockEditor({
  block,
  onChange,
}: ActionsBlockEditorProps) {
  const actions = block.actions ?? {};
  const submit = actions.submit ?? {};

  return (
    <div className="grid gap-3">
      <div className="grid gap-1.5">
        <Label className="text-xs">Submit button label</Label>
        <Input
          value={submit.label ?? ''}
          placeholder="Submit"
          onChange={(event) =>
            onChange({
              ...block,
              actions: {
                ...actions,
                submit: {
                  ...submit,
                  label: event.target.value || null,
                },
              },
            })
          }
        />
      </div>
      <p className="text-xs text-muted-foreground">
        Workflow signal payload mappings live on workflow action buttons in
        their host blocks; this block's source determines which actions surface
        here.
      </p>
    </div>
  );
}
