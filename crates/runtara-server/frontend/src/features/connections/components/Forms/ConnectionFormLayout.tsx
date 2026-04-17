import { ReactNode } from 'react';
import { Link } from 'react-router';
import { Loader2, AlertTriangle, ArrowLeft, Save, Trash2 } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';

type ConnectionFormLayoutProps = {
  title: string;
  description?: string;
  metadata?: string[];
  isLoading?: boolean;
  submitLabel: string;
  loadingLabel?: string;
  cancelHref?: string;
  editNotice?: string;
  children: ReactNode;
  integrationIcon?: ReactNode;
  integrationName?: string;
  integrationCategory?: string;
  onDelete?: () => void;
  isDeleting?: boolean;
};

export function ConnectionFormLayout(props: ConnectionFormLayoutProps) {
  const {
    title,
    isLoading,
    submitLabel,
    loadingLabel,
    cancelHref = '/connections',
    editNotice,
    children,
    integrationIcon,
    integrationName,
    integrationCategory,
    onDelete,
    isDeleting,
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
              <div className="flex items-center gap-3">
                {integrationIcon}
                <div>
                  <h1 className="text-lg font-semibold text-slate-900 dark:text-slate-100">
                    {title}
                  </h1>
                  {(integrationName || integrationCategory) && (
                    <div className="flex items-center gap-2">
                      {integrationName && (
                        <span className="text-sm text-slate-500 dark:text-slate-400">
                          {integrationName}
                        </span>
                      )}
                      {integrationName && integrationCategory && (
                        <span className="w-1 h-1 bg-slate-300 rounded-full dark:bg-slate-600" />
                      )}
                      {integrationCategory && (
                        <span className="text-xs text-slate-500 bg-slate-100 px-1.5 py-0.5 rounded dark:bg-slate-700 dark:text-slate-400">
                          {integrationCategory}
                        </span>
                      )}
                    </div>
                  )}
                </div>
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
      <div className="px-6 py-6 max-w-2xl">
        {/* Edit Notice */}
        {editNotice && (
          <div className="flex items-start gap-3 p-3 bg-amber-50 border border-amber-200/60 rounded-lg mb-6 dark:bg-amber-900/20 dark:border-amber-700/40">
            <AlertTriangle className="w-4 h-4 text-amber-600 flex-shrink-0 mt-0.5 dark:text-amber-500" />
            <div>
              <p className="text-sm font-medium text-amber-800 dark:text-amber-300">
                Stored secrets stay hidden
              </p>
              <p className="text-xs text-amber-700 mt-0.5 dark:text-amber-400">
                Enter new values to update them. Leaving fields empty will keep
                the existing values.
              </p>
            </div>
          </div>
        )}

        {/* Form Fields */}
        {children}
      </div>
    </div>
  );
}
