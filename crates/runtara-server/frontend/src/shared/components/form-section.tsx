import { ReactNode } from 'react';
import { LucideIcon } from 'lucide-react';

type FormSectionProps = {
  title: string;
  description?: string;
  icon?: LucideIcon;
  optional?: boolean;
  children: ReactNode;
};

export function FormSection({
  title,
  description,
  icon: Icon,
  optional,
  children,
}: FormSectionProps) {
  return (
    <div className="overflow-hidden rounded-lg border border-slate-200/80 bg-white shadow-sm dark:border-slate-700/50 dark:bg-card">
      <div className="border-b border-slate-100 bg-slate-50/50 px-4 py-3 dark:border-slate-700/50 dark:bg-slate-800/50">
        <div className="flex items-center justify-between">
          <div>
            <h3 className="flex items-center gap-2 text-sm font-medium text-slate-900 dark:text-slate-100">
              {Icon && (
                <Icon className="h-4 w-4 text-slate-500 dark:text-slate-400" />
              )}
              {title}
            </h3>
            {description && (
              <p className="mt-0.5 text-xs text-slate-500 dark:text-slate-400">
                {description}
              </p>
            )}
          </div>
          {optional && (
            <span className="rounded bg-slate-100 px-2 py-0.5 text-xs text-slate-500 dark:bg-slate-700 dark:text-slate-400">
              Optional
            </span>
          )}
        </div>
      </div>
      <div className="space-y-4 p-4">{children}</div>
    </div>
  );
}
