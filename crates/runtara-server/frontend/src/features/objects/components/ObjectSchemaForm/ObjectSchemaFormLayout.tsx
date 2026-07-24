import { ReactNode } from 'react';
import { Link } from 'react-router';
import { ArrowLeft, Save, Trash2, Database } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import { Spinner } from '@/shared/components/ui/spinner';

type ObjectSchemaFormLayoutProps = {
  title: string;
  schemaName?: string;
  isLoading?: boolean;
  submitLabel: string;
  loadingLabel?: string;
  cancelHref?: string;
  children: ReactNode;
  onDelete?: () => void;
  isDeleting?: boolean;
  metadata?: (string | null)[];
};

export function ObjectSchemaFormLayout(props: ObjectSchemaFormLayoutProps) {
  const {
    title,
    schemaName,
    isLoading,
    submitLabel,
    loadingLabel,
    cancelHref = '/objects/types',
    children,
    onDelete,
    isDeleting,
    metadata,
  } = props;

  const isEditMode = title.toLowerCase().includes('edit');

  return (
    <div className="min-h-screen bg-muted/30 dark:bg-background">
      {/* Sticky Header */}
      <header className="sticky top-0 z-10 border-b border-border bg-muted/50 backdrop-blur-sm dark:bg-background/80">
        <div className="px-6 py-4">
          <div className="flex items-center justify-between">
            <div className="flex items-center gap-3">
              <Link
                to={cancelHref}
                className="-ml-1.5 rounded-lg p-1.5 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
              >
                <ArrowLeft className="h-5 w-5" />
              </Link>
              <div>
                <p className="text-xs font-semibold uppercase tracking-wider text-blue-600">
                  Objects
                </p>
                <h1 className="text-lg font-semibold text-foreground">
                  {title}
                  {schemaName && (
                    <span className="ml-1 text-muted-foreground">
                      {schemaName}
                    </span>
                  )}
                </h1>
              </div>
            </div>
            <div className="flex items-center gap-2">
              {isEditMode && onDelete && (
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  onClick={onDelete}
                  disabled={isDeleting}
                  className="text-red-600 hover:bg-red-50 hover:text-red-700 dark:hover:bg-red-900/30"
                >
                  {isDeleting ? (
                    <Spinner className="mr-1.5 h-4 w-4" />
                  ) : (
                    <Trash2 className="mr-1.5 h-4 w-4" />
                  )}
                  Delete
                </Button>
              )}
              <Link to={cancelHref}>
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  className="text-muted-foreground hover:text-foreground"
                >
                  Cancel
                </Button>
              </Link>
              <Button
                type="submit"
                size="sm"
                disabled={isLoading}
                className="shadow-sm shadow-blue-600/20"
              >
                {isLoading ? (
                  <>
                    <Spinner className="mr-1.5 h-4 w-4" />
                    {loadingLabel || 'Saving...'}
                  </>
                ) : (
                  <>
                    <Save className="mr-1.5 h-4 w-4" />
                    {submitLabel}
                  </>
                )}
              </Button>
            </div>
          </div>
        </div>
      </header>

      {/* Form Content */}
      <div className="max-w-4xl px-6 py-5">
        {/* Object Type Header */}
        <div className="mb-5 flex items-center gap-3">
          <div className="flex h-10 w-10 items-center justify-center rounded-lg bg-gradient-to-br from-blue-500 to-indigo-600 shadow-sm">
            <Database className="h-5 w-5 text-white" />
          </div>
          <div>
            <h2 className="text-base font-semibold text-foreground">
              Object Type Definition
            </h2>
            {metadata && metadata.filter(Boolean).length > 0 && (
              <div className="mt-0.5 flex items-center gap-2">
                {metadata.filter(Boolean).map((item, index) => (
                  <span
                    key={index}
                    className="flex items-center gap-2 text-sm text-muted-foreground"
                  >
                    {index > 0 && (
                      <span className="h-1 w-1 rounded-full bg-border" />
                    )}
                    {item}
                  </span>
                ))}
              </div>
            )}
          </div>
        </div>

        {/* Form Fields */}
        {children}
      </div>
    </div>
  );
}
