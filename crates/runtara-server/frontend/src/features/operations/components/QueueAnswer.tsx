import { stateLabel } from '../state-label';
import { inlineOptions } from '../answer-options';
import { Link } from 'react-router';
import { forwardRef, useImperativeHandle, useMemo, useState } from 'react';
import type { OperationRequest } from '@/generated/RuntaraRuntimeApi';
import { Button } from '@/shared/components/ui/button';
import {
  FormRenderer,
  analyzeFormWithRust,
  type FormAnalysisResult,
} from '@/shared/forms';
import {
  useWorkflowFormDefinition,
  initialWorkflowFormValues,
  workflowFormPayload,
} from '@/features/workflows/utils/form-schema-adapter';
import { useManagedInputSubmissions } from '@/features/workflows/hooks/useManagedInputSubmissions';

export interface AnswerController {
  choose(value: unknown): Promise<void>;
  submit(): Promise<void>;
}
export const QueueAnswer = forwardRef<
  AnswerController,
  { row: OperationRequest; inline?: string | null; infer?: boolean }
>(function QueueAnswer({ row, inline, infer }, ref) {
  const { definition, loading, error } = useWorkflowFormDefinition(
    row.inputSchema
  );
  const [values, setValues] = useState<Record<string, unknown>>({});
  const [editing, setEditing] = useState(false);
  const [analysis, setAnalysis] = useState<FormAnalysisResult | null>(null);
  const [attempt, setAttempt] = useState(0);
  const submissions = useManagedInputSubmissions();
  const retained = [...submissions.inputs]
    .reverse()
    .find(
      (input) =>
        input.request.instanceId === row.instanceId &&
        input.request.requestId === row.requestId
    );
  const disabled = Boolean(retained) || loading || Boolean(error);
  const options = inlineOptions(row.inputSchema, inline, infer);
  const current = useMemo(
    () => ({ ...initialWorkflowFormValues(definition), ...values }),
    [definition, values]
  );
  async function send(payload: Record<string, unknown>) {
    if (disabled) return;
    payload = workflowFormPayload(definition, payload);
    const result = await analyzeFormWithRust(definition, payload);
    setAnalysis(result);
    setAttempt((n) => n + 1);
    if (!result.valid) {
      setEditing(true);
      return;
    }
    const visible = Object.fromEntries(
      Object.keys(definition.fields)
        .filter((name) => result.fields[name]?.visible !== false)
        .map((name) => [name, payload[name]])
    );
    await submissions.submit(
      {
        kind: 'execution',
        workflowId: row.workflowId,
        instanceId: row.instanceId,
        requestId: row.requestId,
        payload: visible,
      },
      row.runLabel ?? row.label
    );
  }
  async function choose(value: unknown) {
    if (
      disabled ||
      !options ||
      !options.values.some((v) => JSON.stringify(v) === JSON.stringify(value))
    )
      return;
    const payload = workflowFormPayload(definition, {
      ...current,
      [options.field]: value,
    });
    setValues(payload);
    const result = await analyzeFormWithRust(definition, payload);
    setAnalysis(result);
    const extra = Object.keys(definition.fields).some(
      (name) =>
        name !== options.field &&
        result.fields[name]?.visible !== false &&
        result.fields[name]?.required
    );
    if (extra || !result.valid) {
      setEditing(true);
      return;
    }
    await send(payload);
  }
  useImperativeHandle(ref, () => ({
    choose,
    submit: async () => {
      if (editing) await send(current);
    },
  }));
  if (!infer && !inline)
    return (
      <Link
        className="text-primary"
        to={`/operations/runs/${row.workflowId}/${row.instanceId}`}
      >
        Review
      </Link>
    );
  if (retained)
    return (
      <div role="status" className="max-w-xs text-sm">
        {retained.state === 'accepted'
          ? 'Answered'
          : retained.state === 'submitting'
            ? 'Sending…'
            : retained.state === 'uncertain'
              ? 'Not confirmed — retry below'
              : `Could not answer: ${retained.error ?? 'request is no longer open'}`}
      </div>
    );
  if (error) return <p role="alert">{error}</p>;
  if (loading) return <p>Preparing answer…</p>;
  return (
    <div className="min-w-52 space-y-3">
      {!editing && options ? (
        <div className="flex flex-wrap gap-2">
          {options.values.map((value) => (
            <Button
              key={JSON.stringify(value)}
              size="sm"
              variant="secondary"
              disabled={disabled}
              onClick={() => void choose(value)}
            >
              {stateLabel(String(value))}
            </Button>
          ))}
        </div>
      ) : null}
      {!editing && !options ? (
        <Button
          variant="secondary"
          size="sm"
          onClick={() => {
            setValues(current);
            setEditing(true);
          }}
        >
          Answer in place
        </Button>
      ) : null}
      {editing ? (
        <div className="space-y-3 rounded border bg-background p-3">
          <FormRenderer
            definition={definition}
            value={current}
            onChange={setValues}
            onAnalysisChange={setAnalysis}
            submitAttempt={attempt}
            disabled={disabled}
          />
          <Button
            size="sm"
            disabled={disabled || analysis?.wasmAvailable === false}
            onClick={() => void send(current)}
          >
            Submit answer
          </Button>
        </div>
      ) : null}
    </div>
  );
});
