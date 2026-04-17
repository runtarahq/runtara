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
    <div className="bg-white rounded-lg border border-slate-200/80 shadow-sm overflow-hidden dark:bg-card dark:border-slate-700/50">
      <div className="px-4 py-3 border-b border-slate-100 bg-slate-50/50 dark:border-slate-700/50 dark:bg-slate-800/50">
        <div className="flex items-center justify-between">
          <div>
            <h3 className="text-sm font-medium text-slate-900 flex items-center gap-2 dark:text-slate-100">
              {Icon && (
                <Icon className="w-4 h-4 text-slate-500 dark:text-slate-400" />
              )}
              {title}
            </h3>
            {description && (
              <p className="text-xs text-slate-500 mt-0.5 dark:text-slate-400">
                {description}
              </p>
            )}
          </div>
          {optional && (
            <span className="text-xs text-slate-500 bg-slate-100 px-2 py-0.5 rounded dark:bg-slate-700 dark:text-slate-400">
              Optional
            </span>
          )}
        </div>
      </div>
      <div className="p-4 space-y-4">{children}</div>
    </div>
  );
}
