import { z } from 'zod';
import { Link } from 'react-router';
import { useMemo } from 'react';
import { Loader2 } from 'lucide-react';
import { useForm } from 'react-hook-form';
import { zodResolver } from '@hookform/resolvers/zod';
import * as form from './TriggerItem';
import { Button } from '@/shared/components/ui/button';
import { FormContent } from '@/shared/components/NextForm/form-content';
import { NextForm } from '@/shared/components/NextForm';

export type TriggerSchemaType = z.infer<typeof form.schema>;

const EMPTY_WORKFLOWS: any[] = [];

type Props = {
  title: string;
  description?: string;
  fieldProps?: Record<string, any>;
  initValues?: TriggerSchemaType;
  isLoading?: boolean;
  submitLabel?: string;
  loadingLabel?: string;
  cancelHref?: string;
  /**
   * Trigger types removed from the type selector (e.g. APPLICATION on the
   * create page — the platform never fires Application triggers).
   */
  hiddenTriggerTypes?: string[];
  onSubmit: (data: TriggerSchemaType) => void;
};

const EMPTY_HIDDEN_TYPES: string[] = [];

export function TriggerForm(props: Props) {
  const {
    title,
    description = 'Map this trigger to a workflow and control how it fires.',
    fieldProps,
    initValues,
    isLoading,
    submitLabel = 'Save trigger',
    loadingLabel = 'Saving...',
    cancelHref = '/invocation-triggers',
    hiddenTriggerTypes = EMPTY_HIDDEN_TYPES,
    onSubmit,
  } = props;

  const entireForm = useForm<TriggerSchemaType>({
    resolver: zodResolver(form.schema),
    defaultValues: form.initialValues,
    values: initValues,
  });

  // Watch for changes to the triggerType field
  const triggerType = entireForm.watch('triggerType');
  const workflows = fieldProps?.workflows || EMPTY_WORKFLOWS;

  const connections = useMemo(
    () => fieldProps?.connections || [],
    [fieldProps?.connections]
  );

  // Filter fieldsConfig based on triggerType and inject workflows/connections
  const filteredFieldsConfig = useMemo(() => {
    return form.fieldsConfig
      .filter((field) => {
        // Only include applicationName and eventType fields if triggerType is APPLICATION
        if (field.name === 'applicationName' || field.name === 'eventType') {
          return triggerType === 'APPLICATION';
        }
        // Only include connectionId and sessionMode fields if triggerType is CHANNEL
        if (field.name === 'connectionId' || field.name === 'sessionMode') {
          return triggerType === 'CHANNEL';
        }
        // Only include static inputs and debug mode fields if triggerType is CRON
        if (field.name === 'cronInputs' || field.name === 'cronDebug') {
          return triggerType === 'CRON';
        }
        // Only include webhook debug + verification connection fields if
        // triggerType is HTTP or EMAIL (both ingest via the events handler)
        if (
          field.name === 'webhookDebug' ||
          field.name === 'webhookConnectionId'
        ) {
          return triggerType === 'HTTP' || triggerType === 'EMAIL';
        }
        return true;
      })
      .map((field) => {
        // Inject workflows into the WorkflowField config; CronInputsField
        // also needs them to resolve the selected workflow's input schema
        if (field.name === 'workflowId' || field.name === 'cronInputs') {
          return { ...field, workflows };
        }
        // Optionally hide trigger types (e.g. APPLICATION on create)
        if (field.name === 'triggerType' && hiddenTriggerTypes.length > 0) {
          return {
            ...field,
            options: (field.options as { label: string; value: string }[]).filter(
              (option) => !hiddenTriggerTypes.includes(option.value)
            ),
          };
        }
        // Inject messaging connections into the connectionId select
        if (field.name === 'connectionId') {
          const messagingConnections = connections.filter(
            (c: any) =>
              c.integrationId === 'telegram_bot' ||
              c.integrationId === 'slack_bot' ||
              c.integrationId === 'teams_bot' ||
              c.integrationId === 'mailgun'
          );
          return {
            ...field,
            options: messagingConnections.map((c: any) => ({
              label: `${c.title} (${c.integrationId?.replace('_bot', '')})`,
              value: c.id,
            })),
          };
        }
        return field;
      });
  }, [triggerType, workflows, connections, hiddenTriggerTypes]);

  const handleSubmit = (values: TriggerSchemaType) => {
    onSubmit(values);
  };

  return (
    <NextForm
      form={entireForm}
      fieldsConfig={filteredFieldsConfig}
      onSubmit={handleSubmit}
      className="space-y-6"
      renderContent={() => (
        <section className="rounded-lg bg-card px-4 py-5 shadow-none sm:px-6 sm:py-6">
          <div className="space-y-6">
            <div className="space-y-1">
              <p className="text-sm font-semibold text-foreground">{title}</p>
              {description && (
                <p className="text-sm text-muted-foreground">{description}</p>
              )}
            </div>
            <FormContent
              fieldsConfig={filteredFieldsConfig}
              className="grid-cols-1 gap-5 sm:grid-cols-2 sm:gap-6"
            />
          </div>
        </section>
      )}
      renderActions={() => (
        <div className="flex flex-col gap-3 px-4 sm:flex-row sm:items-center sm:justify-end sm:px-6">
          <Link to={cancelHref} className="w-full sm:w-auto">
            <Button
              type="button"
              variant="ghost"
              className="w-full justify-center text-muted-foreground hover:text-foreground"
            >
              Cancel
            </Button>
          </Link>
          <Button
            type="submit"
            disabled={isLoading}
            className="w-full sm:w-auto"
          >
            {isLoading ? (
              <>
                <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                {loadingLabel}
              </>
            ) : (
              submitLabel
            )}
          </Button>
        </div>
      )}
    />
  );
}
