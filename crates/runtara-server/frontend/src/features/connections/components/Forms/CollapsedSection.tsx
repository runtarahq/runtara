import { useEffect, useState, type ReactNode } from 'react';

type CollapsedSectionProps = {
  label: string;
  description?: string;
  /** Opens the section when it becomes true (e.g. active configuration inside). */
  forceOpen?: boolean;
  children: ReactNode;
};

/**
 * Page-frame twin of the schema renderer's advanced `<details>` section
 * (shared/forms/FormSection.tsx) for domain sections that live outside
 * the FormDefinition, like rate limiting.
 */
export function CollapsedSection({
  label,
  description,
  forceOpen,
  children,
}: CollapsedSectionProps) {
  const [open, setOpen] = useState(Boolean(forceOpen));

  useEffect(() => {
    if (forceOpen) setOpen(true);
  }, [forceOpen]);

  return (
    <details
      open={open}
      onToggle={(event) => setOpen(event.currentTarget.open)}
      className="group rounded-lg border border-border/70 bg-card px-4 py-3"
    >
      <summary className="cursor-pointer list-none font-medium">
        {label}
        {description && (
          <span className="mt-1 block text-xs font-normal text-muted-foreground">
            {description}
          </span>
        )}
      </summary>
      <div className="mt-4">{children}</div>
    </details>
  );
}
