import { ReactNode } from 'react';
import { Link } from 'react-router';
import { Loader2, ArrowLeft, Save, Trash2, Database } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';

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
    <div className="min-h-screen bg-slate-50/50 dark:bg-background">
      {/* Sticky Header */}
      <header className="sticky top-0 z-10 bg-slate-50/80 backdrop-blur-sm border-b border-slate-200/60 dark:bg-background/80 dark:border-slate-700/60">
        <div className="px-6 py-4">
          <div className="flex items-center justify-between">
            <div className="flex items-center gap-3">
              <Link
                to={cancelHref}
                className="p-1.5 -ml-1.5 text-slate-400 hover:text-slate-600 hover:bg-slate-100 rounded-lg transition-colors dark:hover:text-slate-300 dark:hover:bg-slate-800"
              >
                <ArrowLeft className="w-5 h-5" />
              </Link>
              <div>
                <p className="text-xs font-semibold text-blue-600 uppercase tracking-wider">
                  Objects
                </p>
                <h1 className="text-lg font-semibold text-slate-900 dark:text-slate-100">
                  {title}
                  {schemaName && (
                    <span className="text-slate-400 ml-1">{schemaName}</span>
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
                  className="text-red-600 hover:text-red-700 hover:bg-red-50 dark:hover:bg-red-900/30"
                >
                  {isDeleting ? (
                    <Loader2 className="w-4 h-4 mr-1.5 animate-spin" />
                  ) : (
                    <Trash2 className="w-4 h-4 mr-1.5" />
                  )}
                  Delete
                </Button>
              )}
              <Link to={cancelHref}>
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  className="text-slate-600 hover:text-slate-800 dark:text-slate-400 dark:hover:text-slate-200"
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
                    <Loader2 className="w-4 h-4 mr-1.5 animate-spin" />
                    {loadingLabel || 'Saving...'}
                  </>
                ) : (
                  <>
                    <Save className="w-4 h-4 mr-1.5" />
                    {submitLabel}
                  </>
                )}
              </Button>
            </div>
          </div>
        </div>
      </header>

      {/* Form Content */}
      <div className="px-6 py-5 max-w-4xl">
        {/* Object Type Header */}
        <div className="flex items-center gap-3 mb-5">
          <div className="w-10 h-10 rounded-lg bg-gradient-to-br from-blue-500 to-indigo-600 flex items-center justify-center shadow-sm">
            <Database className="w-5 h-5 text-white" />
          </div>
          <div>
            <h2 className="text-base font-semibold text-slate-900 dark:text-slate-100">
              Object Type Definition
            </h2>
            {metadata && metadata.filter(Boolean).length > 0 && (
              <div className="flex items-center gap-2 mt-0.5">
                {metadata.filter(Boolean).map((item, index) => (
                  <span
                    key={index}
                    className="text-sm text-slate-500 dark:text-slate-400 flex items-center gap-2"
                  >
                    {index > 0 && (
                      <span className="w-1 h-1 bg-slate-300 rounded-full dark:bg-slate-600" />
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
