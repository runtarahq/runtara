import { Link } from 'react-router';
import { useForm } from 'react-hook-form';
import { z } from 'zod';
import { Loader2 } from 'lucide-react';
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
    <div className="w-full px-4 py-6 sm:px-6 lg:px-10">
      <div className="mx-auto w-full max-w-5xl">
        <div className="space-y-2 px-4 sm:px-6">
          <p className="text-xs font-semibold uppercase tracking-[0.08em] text-muted-foreground">
            Workflows
          </p>
          <h1 className="text-3xl font-semibold leading-tight text-slate-900/90">
            {title}
          </h1>
        </div>

        <Form {...form}>
          <form onSubmit={form.handleSubmit(onSubmit)}>
            <div className="mt-6 rounded-2xl bg-card px-4 py-5 shadow-none sm:px-6 sm:py-6">
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
                    <Loader2 className="mr-2 h-4 w-4 animate-spin" />
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
    </div>
  );
}
