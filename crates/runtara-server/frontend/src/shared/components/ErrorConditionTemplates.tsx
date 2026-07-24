import { useState } from 'react';
import { Button } from '@/shared/components/ui/button';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/shared/components/ui/dropdown-menu';
import { AlertTriangle, ChevronDown } from 'lucide-react';
import { ERROR_CONDITION_TEMPLATES } from '@/shared/constants/error-condition-templates';
import type { Condition } from '@/shared/components/ui/condition-editor';

interface ErrorConditionTemplatesProps {
  onSelect: (condition: Condition) => void;
  disabled?: boolean;
}

/**
 * Quick-select templates for error handling conditions.
 * Displays pre-built conditions for common error handling patterns using the __error context.
 *
 * @see docs/structured-errors.md for __error context documentation
 */
export function ErrorConditionTemplates({
  onSelect,
  disabled = false,
}: ErrorConditionTemplatesProps) {
  const [open, setOpen] = useState(false);

  const handleSelect = (condition: Condition) => {
    onSelect(condition);
    setOpen(false);
  };

  return (
    <DropdownMenu open={open} onOpenChange={setOpen}>
      <DropdownMenuTrigger asChild>
        <Button
          variant="outline"
          size="sm"
          disabled={disabled}
          className="gap-1.5 text-xs"
        >
          <AlertTriangle className="h-3.5 w-3.5 text-destructive" />
          Error Templates
          <ChevronDown className="h-3 w-3 opacity-50" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent
        align="start"
        className="max-h-80 w-72 overflow-y-auto"
      >
        <DropdownMenuLabel className="text-xs text-muted-foreground">
          Quick Error Conditions
        </DropdownMenuLabel>
        <DropdownMenuSeparator />
        {ERROR_CONDITION_TEMPLATES.map((template, index) => (
          <DropdownMenuItem
            key={index}
            onClick={() => handleSelect(template.condition)}
            className="cursor-pointer flex-col items-start gap-0.5 py-2"
          >
            <div className="text-sm font-medium">{template.label}</div>
            <div className="text-[11px] leading-snug text-muted-foreground">
              {template.description}
            </div>
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
