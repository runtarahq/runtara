import { Link } from 'react-router';
import { useForm } from 'react-hook-form';
import { z } from 'zod';
import {
  Form,
  FormControl,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from '@/shared/components/ui/form.tsx';
import { Button } from '@/shared/components/ui/button.tsx';
import { zodResolver } from '@hookform/resolvers/zod';
import { Input } from '@/shared/components/ui/input.tsx';
import { Spinner } from '@/shared/components/ui/spinner';
import { PageContainer } from '@/shared/components/page-container';
import { SectionLabel } from '@/shared/components/section-label';

interface WorkflowFormProps {
  title: string;
  loading?: boolean;
  onSubmit: (v: any) => void;
}

export function WorkflowForm(props: WorkflowFormProps) {
  const { title, loading, onSubmit } = props;

  const schema = z.object({
    name: z.string().min(1, 'Workflow name is required'),
  });

  type SchemaType = z.infer<typeof schema>;

  const form = useForm<SchemaType>({
    resolver: zodResolver(schema),
    defaultValues: {
      name: '',
    },
  });

  return (
    <PageContainer>
      <div className="mx-auto w-full max-w-5xl">
        <div className="space-y-2 px-4 sm:px-6">
          <SectionLabel>Workflows</SectionLabel>
          <h1 className="text-3xl font-semibold leading-tight text-foreground">
            {title}
          </h1>
        </div>

        <Form {...form}>
          <form onSubmit={form.handleSubmit(onSubmit)}>
            <div className="mt-6 rounded-lg bg-card px-4 py-5 shadow-none sm:px-6 sm:py-6">
              <FormField
                name="name"
                render={({ field }) => (
                  <FormItem>
                    <FormLabel>Name</FormLabel>
                    <FormControl>
                      <Input {...field} />
                    </FormControl>
                    <FormMessage />
                  </FormItem>
                )}
              />
            </div>

            <div className="mt-8 flex flex-col gap-3 px-4 sm:flex-row sm:items-center sm:justify-end sm:px-6">
              <Link to="/workflows" className="w-full sm:w-auto">
                <Button
                  type="button"
                  variant="ghost"
                  disabled={loading}
                  className="w-full justify-center text-muted-foreground hover:text-foreground"
                >
                  Cancel
                </Button>
              </Link>
              <Button
                type="submit"
                disabled={loading}
                className="w-full sm:w-auto"
              >
                {loading ? (
                  <>
                    <Spinner className="mr-2 h-4 w-4" />
                    Saving...
                  </>
                ) : (
                  'Save'
                )}
              </Button>
            </div>
          </form>
        </Form>
      </div>
    </PageContainer>
  );
}
