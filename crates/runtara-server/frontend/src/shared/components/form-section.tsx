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
    <div className="overflow-hidden rounded-lg border border-border bg-card shadow-sm">
      <div className="border-b border-border bg-muted/30 px-4 py-3">
        <div className="flex items-center justify-between">
          <div>
            <h3 className="flex items-center gap-2 text-sm font-medium text-foreground">
              {Icon && <Icon className="h-4 w-4 text-muted-foreground" />}
              {title}
            </h3>
            {description && (
              <p className="mt-0.5 text-xs text-muted-foreground">
                {description}
              </p>
            )}
          </div>
          {optional && (
            <span className="rounded bg-muted px-2 py-0.5 text-xs text-muted-foreground">
              Optional
            </span>
          )}
        </div>
      </div>
      <div className="space-y-4 p-4">{children}</div>
    </div>
  );
}
