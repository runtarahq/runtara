import { z } from 'zod';

export const MAX_RUN_LABEL_LENGTH = 250;
export const RUN_LABEL_HELP =
  'Letters A-Z, numbers, spaces, dots, dashes, /, ( ), and [ ]. Must contain at least one letter or digit. Long labels are truncated to 250 characters. Invalid resolved labels are ignored; Finish still completes.';

export const runLabelSchema = z
  .object({
    valueType: z.enum(['immediate', 'reference', 'template']),
    value: z.string().nullable(),
  })
  .passthrough()
  .superRefine((label, ctx) => {
    if (label.valueType !== 'immediate') {
      if (!label.value?.trim()) {
        ctx.addIssue({
          code: z.ZodIssueCode.custom,
          message: 'Enter a reference or template',
          path: ['value'],
        });
      }
      return;
    }
    const value = (label.value ?? '').replace(/^ +| +$/g, '');
    if (
      !/^[A-Za-z0-9 ./()[\]-]*$/.test(value) ||
      (label.value != null &&
        label.value !== '' &&
        !/[A-Za-z0-9]/.test(value.slice(0, MAX_RUN_LABEL_LENGTH)))
    ) {
      ctx.addIssue({
        code: z.ZodIssueCode.custom,
        message: RUN_LABEL_HELP,
        path: ['value'],
      });
    }
  });

export function executionDisplayName(run: {
  runLabel?: string | null;
  workflowName?: string | null;
  workflowId?: string | null;
}) {
  return (
    run.runLabel || run.workflowName || run.workflowId || 'Ad-hoc invocation'
  );
}
