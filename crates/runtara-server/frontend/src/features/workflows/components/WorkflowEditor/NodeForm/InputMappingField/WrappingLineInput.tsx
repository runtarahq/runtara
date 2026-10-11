import { useLayoutEffect, useRef } from 'react';
import { Textarea } from '@/shared/components/ui/textarea';
import { cn } from '@/lib/utils';

/** Past this the field scrolls instead of growing. */
const MAX_HEIGHT_PX = 160;

/**
 * A single-line value that wraps instead of scrolling sideways.
 *
 * In the 520px step panel a value cell is ~150px wide, so an `<input>` showed
 * the first 15 characters of a URL template or an expression and hid the
 * rest. This is a one-row textarea that grows with its wrapped content, but
 * keeps single-line semantics: Enter inserts nothing and pasted newlines are
 * dropped, as a text input would.
 */
export function WrappingLineInput({
  value,
  onChange,
  placeholder,
  disabled,
  className,
}: {
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  disabled?: boolean;
  className?: string;
}) {
  const ref = useRef<HTMLTextAreaElement | null>(null);

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.style.height = 'auto';
    el.style.height = `${Math.min(el.scrollHeight, MAX_HEIGHT_PX)}px`;
  }, [value]);

  return (
    <Textarea
      ref={ref}
      rows={1}
      value={value}
      onChange={(e) => onChange(e.target.value.replace(/[\r\n]+/g, ''))}
      onKeyDown={(e) => {
        if (e.key === 'Enter') e.preventDefault();
      }}
      placeholder={placeholder}
      disabled={disabled}
      aria-multiline={false}
      className={cn(
        'min-h-9 flex-1 resize-none overflow-y-auto border-0 py-2 leading-5 shadow-none focus-visible:ring-0 focus-visible:ring-offset-0',
        className
      )}
    />
  );
}
