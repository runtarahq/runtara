import { useMemo } from 'react';
import { ConditionEditor } from '@/shared/components/ui/condition-editor';
import { Label } from '@/shared/components/ui/label';
import { ReportRowCondition } from '../../../types';
import {
  canonicalToLegacyCondition,
  legacyToCanonicalCondition,
} from '../../../utils';

interface VisibilityEditorProps {
  label: string;
  description?: string;
  condition: ReportRowCondition | null | undefined;
  onChange: (condition: ReportRowCondition | undefined) => void;
}

/** Wraps the workflow-shared ConditionEditor for canonical
 *  `ConditionExpression` fields (showWhen / visibleWhen / disabledWhen).
 *  Translates to/from the editor's legacy shape via the bridge helpers
 *  already in reports/utils.ts. */
export function VisibilityEditor({
  label,
  description,
  condition,
  onChange,
}: VisibilityEditorProps) {
  const legacyValue = useMemo(() => {
    const legacy = canonicalToLegacyCondition(condition);
    return legacy ? JSON.stringify(legacy) : undefined;
  }, [condition]);

  return (
    <div className="grid gap-1.5">
      <Label className="text-xs">{label}</Label>
      {description ? (
        <p className="text-xs text-muted-foreground">{description}</p>
      ) : null}
      <ConditionEditor
        value={legacyValue}
        onChange={(value) => {
          if (!value) {
            onChange(undefined);
            return;
          }
          try {
            const parsed = JSON.parse(value);
            const canonical = legacyToCanonicalCondition(parsed);
            onChange(canonical);
          } catch {
            // Editor returned a bad payload — keep the previous value rather
            // than dropping the user's work.
          }
        }}
      />
    </div>
  );
}
