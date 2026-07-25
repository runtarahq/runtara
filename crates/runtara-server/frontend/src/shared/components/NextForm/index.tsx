import { toast } from 'sonner';
import { cn } from '@/lib/utils.ts';
import { Form } from '@/shared/components/ui/form.tsx';
import { FormContent } from './form-content.tsx';
import { describeFirstFormError } from './form-errors.ts';

interface Props {
  className?: string;
  form: any;
  fieldsConfig?: Record<string, any>[];
  formProps?: Record<string, any>;
  renderHeader?: () => React.ReactNode;
  renderContent?: () => React.ReactNode;
  renderActions?: () => React.ReactNode;
  renderButtons?: () => React.ReactNode;
  onSubmit: (data: any) => void;
  /**
   * Called when submit is blocked by validation. Defaults to reporting the
   * first failing field and focusing it — without this, `handleSubmit` swallows
   * the failure and the submit button appears to do nothing at all.
   */
  onInvalid?: (errors: Record<string, any>) => void;
}

export function NextForm(props: Props) {
  const {
    className,
    form,
    fieldsConfig,
    formProps = {},
    renderButtons = () => null,
    onSubmit,
    onInvalid,
  } = props;

  const {
    renderHeader = () => null,
    renderContent = () => <FormContent fieldsConfig={fieldsConfig} />,
    renderActions = () => renderButtons(),
  } = props;

  const handleInvalid = (errors: Record<string, any>) => {
    if (onInvalid) {
      onInvalid(errors);
      return;
    }
    const first = describeFirstFormError(errors);
    if (!first) return;
    toast.error(first.message, { description: first.label });
    // Bring the offending control into view; not every path maps to a
    // registered input (nested editors render their own controls), so this is
    // best-effort.
    try {
      form.setFocus?.(first.path);
    } catch {
      // No registered field at that path — the toast still names it.
    }
  };

  return (
    <Form {...form} {...formProps}>
      <form
        className={cn(className)}
        onSubmit={form.handleSubmit(onSubmit, handleInvalid)}
      >
        {renderHeader()}
        {renderContent()}
        {renderActions()}
      </form>
    </Form>
  );
}

/*
<Dialog>
  <NextForm>
    <form>
      <Dialog /> or <Card />
    </form>
  </NextForm>
</Dialog>
*/

/*
<Dialog>
  <DialogContent>
    <DialogHeader></DialogHeader>
    <DialogFooter></DialogFooter>
  </DialogContent>
</Dialog>
*/

/*
<Card>
  <CardHeader></CardHeader>
  <CardContent></CardContent>
  <CardFooter></CardFooter>
</Card>
*/
