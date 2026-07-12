import type { ReactNode } from 'react';

import type { FormSectionDefinition } from './types';

interface FormSectionProps {
  section?: FormSectionDefinition;
  children: ReactNode;
}

export function FormSection({ section, children }: FormSectionProps) {
  if (!section) {
    return <div className="space-y-4">{children}</div>;
  }

  const content = (
    <div className="space-y-4 border-t border-border/60 pt-4">{children}</div>
  );

  if (section.advanced) {
    return (
      <details className="group rounded-lg border border-border/70 bg-card px-4 py-3">
        <summary className="cursor-pointer list-none font-medium">
          {section.label}
          {section.description && (
            <span className="mt-1 block text-xs font-normal text-muted-foreground">
              {section.description}
            </span>
          )}
        </summary>
        <div className="mt-4">{content}</div>
      </details>
    );
  }

  return (
    <section className="rounded-lg border border-border/70 bg-card px-4 py-4">
      <div className="mb-4">
        <h3 className="font-medium">{section.label}</h3>
        {section.description && (
          <p className="mt-1 text-xs text-muted-foreground">
            {section.description}
          </p>
        )}
      </div>
      {content}
    </section>
  );
}
