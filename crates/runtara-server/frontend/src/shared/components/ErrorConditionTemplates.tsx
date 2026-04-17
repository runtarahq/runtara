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
          className="text-xs gap-1.5"
        >
          <AlertTriangle className="w-3.5 h-3.5 text-destructive" />
          Error Templates
          <ChevronDown className="w-3 h-3 opacity-50" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent
        align="start"
        className="w-72 max-h-80 overflow-y-auto"
      >
        <DropdownMenuLabel className="text-xs text-muted-foreground">
          Quick Error Conditions
        </DropdownMenuLabel>
        <DropdownMenuSeparator />
        {ERROR_CONDITION_TEMPLATES.map((template, index) => (
          <DropdownMenuItem
            key={index}
            onClick={() => handleSelect(template.condition)}
            className="flex-col items-start gap-0.5 cursor-pointer py-2"
          >
            <div className="font-medium text-sm">{template.label}</div>
            <div className="text-[11px] text-muted-foreground leading-snug">
              {template.description}
            </div>
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
