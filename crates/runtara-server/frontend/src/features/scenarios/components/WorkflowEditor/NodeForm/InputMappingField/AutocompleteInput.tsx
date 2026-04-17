import { useContext, useRef, useState } from 'react';
import { Input } from '@/shared/components/ui/input';
import { Textarea } from '@/shared/components/ui/textarea';
import {
  composeVariableSuggestions,
  filterSuggestions,
  groupSuggestions,
  VariableSuggestion,
} from '../InputMappingValueField/VariableSuggestions';
import { NodeFormContext } from '../NodeFormContext';

interface AutocompleteInputProps {
  value?: string;
  onChange?: (value: string) => void;
  placeholder?: string;
  type?: 'text' | 'number' | 'textarea';
  className?: string;
  typeHint?: string;
}

export function AutocompleteInput({
  value = '',
  onChange,
  placeholder,
  type = 'text',
  className,
  typeHint,
}: AutocompleteInputProps) {
  const { previousSteps } = useContext(NodeFormContext);
  const inputRef = useRef<HTMLInputElement | HTMLTextAreaElement>(null);

  // Autocomplete state
  const [showAutocomplete, setShowAutocomplete] = useState(false);
  const [autocompleteQuery, setAutocompleteQuery] = useState('');
  const [triggerIndex, setTriggerIndex] = useState(-1);

  // Don't show autocomplete for "raw" type hint
  const shouldShowAutocomplete = typeHint !== 'raw';

  // Handle input change and detect "{{" trigger
  const handleChange = (
    e: React.ChangeEvent<HTMLInputElement | HTMLTextAreaElement>
  ) => {
    const newValue = e.target.value;
    onChange?.(newValue);

    if (!shouldShowAutocomplete) return;

    // Use setTimeout to ensure cursor position is updated
    setTimeout(() => {
      const element = inputRef.current;
      if (!element) return;

      const cursorPos = element.selectionStart || 0;
      const textBeforeCursor = newValue.substring(0, cursorPos);

      // Find the last occurrence of "{{" before cursor
      const lastTriggerIndex = textBeforeCursor.lastIndexOf('{{');

      if (lastTriggerIndex !== -1) {
        // Check if we're still inside the variable reference (no closing }})
        const textAfterTrigger = textBeforeCursor.substring(lastTriggerIndex);
        const hasClosing = textAfterTrigger.includes('}}');

        if (!hasClosing) {
          // Extract the query after "{{"
          const query = textAfterTrigger.substring(2);

          setAutocompleteQuery(query);
          setTriggerIndex(lastTriggerIndex);
          setShowAutocomplete(true);
          return;
        }
      }

      // If we get here, close autocomplete
      setShowAutocomplete(false);
      setAutocompleteQuery('');
      setTriggerIndex(-1);
    }, 0);
  };

  // Handle autocomplete selection
  const handleAutocompleteSelect = (suggestion: VariableSuggestion) => {
    if (!onChange || triggerIndex === -1) {
      return;
    }

    const element = inputRef.current;
    if (!element) {
      return;
    }

    const cursorPos = element.selectionStart || 0;

    // Replace from "{{" to cursor position with the selected variable
    const beforeTrigger = value.substring(0, triggerIndex);
    const afterCursor = value.substring(cursorPos);
    const newValue = `${beforeTrigger}{{${suggestion.value}}}${afterCursor}`;

    onChange(newValue);

    // Close autocomplete
    setShowAutocomplete(false);
    setAutocompleteQuery('');
    setTriggerIndex(-1);

    // Set cursor position after the inserted variable
    setTimeout(() => {
      if (element) {
        const newCursorPos = beforeTrigger.length + suggestion.value.length + 4; // 4 for {{ and }}
        element.selectionStart = newCursorPos;
        element.selectionEnd = newCursorPos;
        element.focus();
      }
    }, 0);
  };

  // Generate and filter suggestions
  const allSuggestions = composeVariableSuggestions(previousSteps);
  const filteredSuggestions = filterSuggestions(
    allSuggestions,
    autocompleteQuery
  );
  const groupedSuggestions = groupSuggestions(filteredSuggestions);

  // Add visual styling for raw mode
  const rawModeClass =
    typeHint === 'raw'
      ? 'bg-amber-50/50 dark:bg-amber-950/20 border-amber-200 dark:border-amber-900/50'
      : '';

  const inputProps = {
    ref: inputRef as any,
    value,
    onChange: handleChange,
    placeholder,
    className: `${className} ${rawModeClass}`,
  };

  return (
    <div className="relative">
      {type === 'textarea' ? (
        <Textarea
          {...inputProps}
          className={`${className} ${rawModeClass} font-mono text-sm min-h-[100px]`}
        />
      ) : (
        <Input {...inputProps} type={type} />
      )}

      {/* Autocomplete Popover */}
      {showAutocomplete && shouldShowAutocomplete && (
        <div className="absolute left-0 top-full mt-1 w-80 z-50 rounded-md border bg-popover text-popover-foreground shadow-md overflow-hidden">
          <div className="overflow-y-auto max-h-[300px] p-1">
            {filteredSuggestions.length === 0 ? (
              <div className="py-6 text-center text-sm">
                No variables found.
              </div>
            ) : (
              <>
                {groupedSuggestions['Scenario Inputs'].length > 0 && (
                  <div className="mb-2">
                    <div className="px-2 py-1.5 text-xs font-semibold text-muted-foreground">
                      Scenario Inputs
                    </div>
                    {groupedSuggestions['Scenario Inputs'].map((suggestion) => (
                      <div
                        key={suggestion.value}
                        onClick={() => handleAutocompleteSelect(suggestion)}
                        className="relative flex cursor-pointer select-none items-center rounded-sm px-2 py-1.5 text-sm outline-none hover:bg-accent hover:text-accent-foreground transition-colors"
                      >
                        <div className="flex flex-col">
                          <span className="font-mono text-sm">
                            {suggestion.label}
                          </span>
                          {suggestion.description && (
                            <span className="text-xs text-muted-foreground">
                              {suggestion.description}
                            </span>
                          )}
                        </div>
                      </div>
                    ))}
                  </div>
                )}

                {groupedSuggestions['Step Outputs'].length > 0 && (
                  <div>
                    <div className="px-2 py-1.5 text-xs font-semibold text-muted-foreground">
                      Step Outputs
                    </div>
                    {groupedSuggestions['Step Outputs'].map((suggestion) => (
                      <div
                        key={suggestion.value}
                        onClick={() => handleAutocompleteSelect(suggestion)}
                        className="relative flex cursor-pointer select-none items-center rounded-sm px-2 py-1.5 text-sm outline-none hover:bg-accent hover:text-accent-foreground transition-colors"
                      >
                        <div className="flex flex-col">
                          <span className="font-mono text-sm">
                            {suggestion.label}
                          </span>
                          {suggestion.description && (
                            <span className="text-xs text-muted-foreground">
                              {suggestion.description}
                              {suggestion.type && ` • ${suggestion.type}`}
                            </span>
                          )}
                        </div>
                      </div>
                    ))}
                  </div>
                )}
              </>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
