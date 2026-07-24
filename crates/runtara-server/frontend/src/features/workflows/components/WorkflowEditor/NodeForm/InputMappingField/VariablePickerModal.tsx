import { useState, useContext, useMemo } from 'react';
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
} from '@/shared/components/ui/dialog';
import { Input } from '@/shared/components/ui/input';
import { Icons } from '@/shared/components/icons.tsx';
import { NodeFormContext } from '../NodeFormContext';
import {
  composeVariableSuggestions,
  filterSuggestions,
  groupSuggestions,
  VariableSuggestion,
} from '../InputMappingValueField/VariableSuggestions';

interface VariablePickerModalProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onSelect: (variable: VariableSuggestion) => void;
}

/**
 * Get icon component based on variable type and path
 */
function getIconForType(type?: string, path?: string) {
  const lowerType = type?.toLowerCase() || '';
  const lowerPath = path?.toLowerCase() || '';

  // Check explicit type first
  if (lowerType.includes('string') || lowerType.includes('text')) {
    return <Icons.type className="h-4 w-4" />;
  }
  if (
    lowerType.includes('number') ||
    lowerType.includes('int') ||
    lowerType.includes('integer') ||
    lowerType.includes('double') ||
    lowerType.includes('float')
  ) {
    return <Icons.hash className="h-4 w-4" />;
  }
  if (lowerType.includes('boolean') || lowerType.includes('bool')) {
    return <Icons.squareCheck className="h-4 w-4" />;
  }
  if (lowerType.includes('array') || lowerType.includes('list')) {
    return <Icons.list className="h-4 w-4" />;
  }
  if (lowerType.includes('object')) {
    return <Icons.braces className="h-4 w-4" />;
  }
  if (
    lowerType.includes('date') ||
    lowerType.includes('time') ||
    lowerPath.includes('date') ||
    lowerPath.includes('time')
  ) {
    return <Icons.calendar className="h-4 w-4" />;
  }

  // Infer from path
  if (lowerPath.includes('email')) {
    return <Icons.mail className="h-4 w-4" />;
  }
  if (lowerPath.includes('name')) {
    return <Icons.user className="h-4 w-4" />;
  }
  if (lowerPath.includes('id') || lowerPath.includes('key')) {
    return <Icons.key className="h-4 w-4" />;
  }
  if (
    lowerPath.includes('price') ||
    lowerPath.includes('amount') ||
    lowerPath.includes('total') ||
    lowerPath.includes('cost')
  ) {
    return <Icons.dollarSign className="h-4 w-4" />;
  }

  // Default icon
  return <Icons.gitBranch className="h-4 w-4" />;
}

/**
 * Modal dialog for browsing and selecting available variables from previous steps
 */
export function VariablePickerModal({
  open,
  onOpenChange,
  onSelect,
}: VariablePickerModalProps) {
  const [searchQuery, setSearchQuery] = useState('');
  const {
    previousSteps,
    inputSchemaFields,
    variables,
    isInsideWhileLoop,
    isInsideSplit,
    isInsideWaitScope,
    splitItemSchemaFields,
  } = useContext(NodeFormContext);

  // Generate and filter suggestions
  const allSuggestions = useMemo(
    () =>
      composeVariableSuggestions(
        previousSteps,
        inputSchemaFields,
        variables,
        isInsideWhileLoop,
        isInsideSplit,
        isInsideWaitScope,
        splitItemSchemaFields
      ),
    [
      previousSteps,
      inputSchemaFields,
      variables,
      isInsideWhileLoop,
      isInsideSplit,
      isInsideWaitScope,
      splitItemSchemaFields,
    ]
  );

  const filteredSuggestions = useMemo(
    () => filterSuggestions(allSuggestions, searchQuery),
    [allSuggestions, searchQuery]
  );

  const groupedSuggestions = useMemo(
    () => groupSuggestions(filteredSuggestions),
    [filteredSuggestions]
  );

  const handleSelect = (suggestion: VariableSuggestion) => {
    onSelect(suggestion);
    onOpenChange(false);
    setSearchQuery('');
  };

  const handleOpenChange = (newOpen: boolean) => {
    onOpenChange(newOpen);
    if (!newOpen) {
      setSearchQuery('');
    }
  };

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent className="sm:max-w-[500px]">
        <DialogHeader>
          <DialogTitle>Select Variable</DialogTitle>
          <DialogDescription>
            Choose a variable from workflow inputs or previous step outputs
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-4">
          {/* Search input */}
          <div className="relative">
            <Icons.search className="absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
            <Input
              placeholder="Search variables..."
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
              className="pl-9 font-mono text-sm"
              autoFocus
            />
          </div>

          {/* Variable list */}
          <div className="max-h-[400px] space-y-4 overflow-y-auto">
            {/* Free-text path entry: any legal reference path can be used
                even when it is not in the suggestion list */}
            {searchQuery.trim() !== '' &&
              !allSuggestions.some(
                (suggestion) => suggestion.value === searchQuery.trim()
              ) && (
                <button
                  type="button"
                  onClick={() =>
                    handleSelect({
                      label: searchQuery.trim(),
                      value: searchQuery.trim(),
                      group: 'Workflow Inputs',
                    })
                  }
                  className="flex w-full items-center gap-2 rounded border border-dashed px-2 py-1.5 text-left text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
                >
                  <div className="min-w-0 flex-1">
                    <p className="truncate font-mono text-sm">
                      {searchQuery.trim()}
                    </p>
                    <p className="truncate text-xs opacity-70">
                      Use as custom reference path
                    </p>
                  </div>
                </button>
              )}
            {filteredSuggestions.length === 0 ? (
              <div className="py-8 text-center text-muted-foreground">
                <Icons.inbox className="mx-auto mb-2 h-8 w-8 opacity-50" />
                <p>No variables found</p>
              </div>
            ) : (
              <>
                {/* Uniform iteration context (Split and While scopes) */}
                {groupedSuggestions['Iteration Context'].length > 0 && (
                  <div>
                    <h4 className="mb-2 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
                      Iteration Context
                    </h4>
                    <div className="space-y-0.5">
                      {groupedSuggestions['Iteration Context'].map(
                        (suggestion) => (
                          <button
                            key={suggestion.value}
                            type="button"
                            onClick={() => handleSelect(suggestion)}
                            className="flex w-full items-center gap-2 rounded px-2 py-1.5 text-left text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
                          >
                            {getIconForType(suggestion.type, suggestion.value)}
                            <div className="min-w-0 flex-1">
                              <p className="truncate font-mono text-sm">
                                {suggestion.label}
                              </p>
                              {suggestion.description && (
                                <p className="truncate text-xs opacity-70">
                                  {suggestion.description}
                                </p>
                              )}
                            </div>
                            {suggestion.type && (
                              <span className="shrink-0 rounded bg-black/5 px-1.5 py-0.5 font-mono text-[11px] text-muted-foreground dark:bg-white/10">
                                {suggestion.type}
                              </span>
                            )}
                          </button>
                        )
                      )}
                    </div>
                  </div>
                )}

                {/* Loop Context (While loop scope) */}
                {groupedSuggestions['Loop Context'].length > 0 && (
                  <div>
                    <h4 className="mb-2 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
                      Loop Context
                    </h4>
                    <div className="space-y-0.5">
                      {groupedSuggestions['Loop Context'].map((suggestion) => (
                        <button
                          key={suggestion.value}
                          type="button"
                          onClick={() => handleSelect(suggestion)}
                          className="flex w-full items-center gap-2 rounded px-2 py-1.5 text-left text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
                        >
                          {getIconForType(suggestion.type, suggestion.value)}
                          <div className="min-w-0 flex-1">
                            <p className="truncate font-mono text-sm">
                              {suggestion.label}
                            </p>
                            {suggestion.description && (
                              <p className="truncate text-xs opacity-70">
                                {suggestion.description}
                              </p>
                            )}
                          </div>
                          {suggestion.type && (
                            <span className="shrink-0 rounded bg-black/5 px-1.5 py-0.5 font-mono text-[11px] text-muted-foreground dark:bg-white/10">
                              {suggestion.type}
                            </span>
                          )}
                        </button>
                      ))}
                    </div>
                  </div>
                )}

                {/* Split Scope (Split iteration variables) */}
                {groupedSuggestions['Split Scope'].length > 0 && (
                  <div>
                    <h4 className="mb-2 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
                      Split Scope
                    </h4>
                    <div className="space-y-0.5">
                      {groupedSuggestions['Split Scope'].map((suggestion) => (
                        <button
                          key={suggestion.value}
                          type="button"
                          onClick={() => handleSelect(suggestion)}
                          className="flex w-full items-center gap-2 rounded px-2 py-1.5 text-left text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
                        >
                          {getIconForType(suggestion.type, suggestion.value)}
                          <div className="min-w-0 flex-1">
                            <p className="truncate font-mono text-sm">
                              {suggestion.label}
                            </p>
                            {suggestion.description && (
                              <p className="truncate text-xs opacity-70">
                                {suggestion.description}
                              </p>
                            )}
                          </div>
                          {suggestion.type && (
                            <span className="shrink-0 rounded bg-black/5 px-1.5 py-0.5 font-mono text-[11px] text-muted-foreground dark:bg-white/10">
                              {suggestion.type}
                            </span>
                          )}
                        </button>
                      ))}
                    </div>
                  </div>
                )}

                {/* Wait Scope (WaitForSignal onWait variables) */}
                {groupedSuggestions['Wait Scope'].length > 0 && (
                  <div>
                    <h4 className="mb-2 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
                      Wait Scope
                    </h4>
                    <div className="space-y-0.5">
                      {groupedSuggestions['Wait Scope'].map((suggestion) => (
                        <button
                          key={suggestion.value}
                          type="button"
                          onClick={() => handleSelect(suggestion)}
                          className="flex w-full items-center gap-2 rounded px-2 py-1.5 text-left text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
                        >
                          {getIconForType(suggestion.type, suggestion.value)}
                          <div className="min-w-0 flex-1">
                            <p className="truncate font-mono text-sm">
                              {suggestion.label}
                            </p>
                            {suggestion.description && (
                              <p className="truncate text-xs opacity-70">
                                {suggestion.description}
                              </p>
                            )}
                          </div>
                          {suggestion.type && (
                            <span className="shrink-0 rounded bg-black/5 px-1.5 py-0.5 font-mono text-[11px] text-muted-foreground dark:bg-white/10">
                              {suggestion.type}
                            </span>
                          )}
                        </button>
                      ))}
                    </div>
                  </div>
                )}

                {/* Workflow Inputs */}
                {groupedSuggestions['Workflow Inputs'].length > 0 && (
                  <div>
                    <h4 className="mb-2 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
                      Workflow Inputs
                    </h4>
                    <div className="space-y-0.5">
                      {groupedSuggestions['Workflow Inputs'].map(
                        (suggestion) => (
                          <button
                            key={suggestion.value}
                            type="button"
                            onClick={() => handleSelect(suggestion)}
                            className="flex w-full items-center gap-2 rounded px-2 py-1.5 text-left text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
                          >
                            {getIconForType(suggestion.type, suggestion.value)}
                            <div className="min-w-0 flex-1">
                              <p className="truncate font-mono text-sm">
                                {suggestion.label}
                              </p>
                              {suggestion.description && (
                                <p className="truncate text-xs opacity-70">
                                  {suggestion.description}
                                </p>
                              )}
                            </div>
                            {suggestion.type && (
                              <span className="shrink-0 rounded bg-black/5 px-1.5 py-0.5 font-mono text-[11px] text-muted-foreground dark:bg-white/10">
                                {suggestion.type}
                              </span>
                            )}
                          </button>
                        )
                      )}
                    </div>
                  </div>
                )}

                {/* Variables */}
                {groupedSuggestions['Variables'].length > 0 && (
                  <div>
                    <h4 className="mb-2 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
                      Variables
                    </h4>
                    <div className="space-y-0.5">
                      {groupedSuggestions['Variables'].map((suggestion) => (
                        <button
                          key={suggestion.value}
                          type="button"
                          onClick={() => handleSelect(suggestion)}
                          className="flex w-full items-center gap-2 rounded px-2 py-1.5 text-left text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
                        >
                          {getIconForType(suggestion.type, suggestion.value)}
                          <div className="min-w-0 flex-1">
                            <p className="truncate font-mono text-sm">
                              {suggestion.label}
                            </p>
                            {suggestion.description && (
                              <p className="truncate text-xs opacity-70">
                                {suggestion.description}
                              </p>
                            )}
                          </div>
                          {suggestion.type && (
                            <span className="shrink-0 rounded bg-black/5 px-1.5 py-0.5 font-mono text-[11px] text-muted-foreground dark:bg-white/10">
                              {suggestion.type}
                            </span>
                          )}
                        </button>
                      ))}
                    </div>
                  </div>
                )}

                {/* Step Outputs */}
                {groupedSuggestions['Step Outputs'].length > 0 && (
                  <div>
                    <h4 className="mb-2 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
                      Step Outputs
                    </h4>
                    <div className="space-y-0.5">
                      {groupedSuggestions['Step Outputs'].map((suggestion) => (
                        <button
                          key={suggestion.value}
                          type="button"
                          onClick={() => handleSelect(suggestion)}
                          className="flex w-full items-center gap-2 rounded px-2 py-1.5 text-left text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
                        >
                          {getIconForType(suggestion.type, suggestion.value)}
                          <div className="min-w-0 flex-1">
                            <p className="truncate text-sm">
                              <span className="font-medium">
                                {suggestion.stepName || suggestion.description}
                              </span>
                              {suggestion.fieldPath && (
                                <span className="text-muted-foreground">
                                  {' → '}
                                  <span className="font-mono">
                                    {suggestion.fieldPath}
                                  </span>
                                </span>
                              )}
                            </p>
                            {suggestion.stepId && (
                              <p className="truncate font-mono text-[11px] opacity-50">
                                {suggestion.stepId}
                              </p>
                            )}
                          </div>
                          {suggestion.type && (
                            <span className="shrink-0 rounded bg-black/5 px-1.5 py-0.5 font-mono text-[11px] dark:bg-white/10">
                              {suggestion.type}
                            </span>
                          )}
                        </button>
                      ))}
                    </div>
                  </div>
                )}
              </>
            )}
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
