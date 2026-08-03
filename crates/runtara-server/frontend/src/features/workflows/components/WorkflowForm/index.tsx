import { useMemo } from 'react';
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
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select.tsx';
import { Spinner } from '@/shared/components/ui/spinner';
import { PageContainer } from '@/shared/components/page-container';
import { SectionLabel } from '@/shared/components/section-label';
import { useFolders } from '@/features/workflows/hooks/useFolders';
import { workflowsListHref } from '@/features/workflows/folder-nav';

interface WorkflowFormProps {
  title: string;
  loading?: boolean;
  /** Folder the workflow is created in; preselected in the folder picker. */
  initialPath?: string;
  onSubmit: (v: any) => void;
}

export function WorkflowForm(props: WorkflowFormProps) {
  const { title, loading, initialPath = '/', onSubmit } = props;

  const { data: foldersData } = useFolders();

  // All known folders, plus the one from the URL if it isn't listed yet
  // (folders only exist server-side once a workflow lives in them).
  const folderPaths = useMemo(() => {
    const paths = (foldersData?.parsed ?? []).map((folder) => folder.path);
    if (initialPath !== '/' && !paths.includes(initialPath)) {
      paths.push(initialPath);
      paths.sort();
    }
    return paths;
  }, [foldersData?.parsed, initialPath]);

  const schema = z.object({
    name: z.string().min(1, 'Workflow name is required'),
    path: z.string(),
  });

  type SchemaType = z.infer<typeof schema>;

  const form = useForm<SchemaType>({
    resolver: zodResolver(schema),
    defaultValues: {
      name: '',
      path: initialPath,
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
            <div className="mt-6 space-y-6 rounded-lg bg-card px-4 py-5 shadow-none sm:px-6 sm:py-6">
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

              <FormField
                name="path"
                render={({ field }) => (
                  <FormItem>
                    <FormLabel>Folder</FormLabel>
                    <Select value={field.value} onValueChange={field.onChange}>
                      <FormControl>
                        <SelectTrigger>
                          <SelectValue />
                        </SelectTrigger>
                      </FormControl>
                      <SelectContent>
                        <SelectItem value="/">Root (All Workflows)</SelectItem>
                        {folderPaths.map((path) => (
                          <SelectItem key={path} value={path}>
                            {path}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                    <FormMessage />
                  </FormItem>
                )}
              />
            </div>

            <div className="mt-8 flex flex-col gap-3 px-4 sm:flex-row sm:items-center sm:justify-end sm:px-6">
              <Link
                to={workflowsListHref(initialPath)}
                className="w-full sm:w-auto"
              >
                <Button
                  type="button"
                  variant="secondary"
                  disabled={loading}
                  className="w-full justify-center"
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
                    <Spinner className="mr-2 size-4" />
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
