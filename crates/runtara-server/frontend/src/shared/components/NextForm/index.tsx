import { cn } from '@/lib/utils.ts';
import { Form } from '@/shared/components/ui/form.tsx';
import { FormContent } from './form-content.tsx';

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
}

export function NextForm(props: Props) {
  const {
    className,
    form,
    fieldsConfig,
    formProps = {},
    renderButtons = () => null,
    onSubmit,
  } = props;

  const {
    renderHeader = () => null,
    renderContent = () => <FormContent fieldsConfig={fieldsConfig} />,
    renderActions = () => renderButtons(),
  } = props;

  return (
    <Form {...form} {...formProps}>
      <form className={cn(className)} onSubmit={form.handleSubmit(onSubmit)}>
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
