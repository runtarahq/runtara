import { useState, useRef, useEffect, useContext, useMemo } from 'react';
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
} from '@/shared/components/ui/dialog';
import { Button } from '@/shared/components/ui/button';
import { Tabs, TabsList, TabsTrigger } from '@/shared/components/ui/tabs';
import { Icons } from '@/shared/components/icons';
import { cn } from '@/lib/utils';
import { NodeFormContext } from '../NodeFormContext';
import {
  useNodeFormStore,
  CompositeObjectValue,
  isCompositeValue,
} from '@/features/workflows/stores/nodeFormStore';
import {
  VariableSuggestion,
  composeVariableSuggestions,
} from '../InputMappingValueField/VariableSuggestions';
import {
  renderTemplatePreview,
  getTemplateStats,
} from './template-preview-utils';

type ViewMode = 'editor' | 'preview' | 'split';

interface TemplateEditorModalProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  value: string;
  onChange: (value: string) => void;
  fieldName?: string;
  placeholder?: string;
}

/**
 * Extract variables from a plain object (when value is stored directly as object)
 */
function extractVariablesFromPlainObject(
  obj: Record<string, unknown>
): VariableSuggestion[] {
  const suggestions: VariableSuggestion[] = [];

  Object.entries(obj).forEach(([varName, varValue]) => {
    let varType = 'any';
    let example = '';

    if (typeof varValue === 'string') {
      varType = 'string';
      example = varValue.length > 20 ? varValue.slice(0, 20) + '...' : varValue;
    } else if (typeof varValue === 'number') {
      varType = Number.isInteger(varValue) ? 'integer' : 'number';
      example = String(varValue);
    } else if (typeof varValue === 'boolean') {
      varType = 'boolean';
      example = String(varValue);
    } else if (Array.isArray(varValue)) {
      varType = 'array';
      example = `[${varValue.length} items]`;
    } else if (typeof varValue === 'object' && varValue !== null) {
      varType = 'object';
      example = '{...}';
    }

    suggestions.push({
      label: varName,
      value: varName,
      description: example || undefined,
      group: 'Variables',
      type: varType,
    });
  });

  return suggestions;
}

/**
 * Extract variables from the "variables" field's composite value
 * The variables field is typically a composite object where keys are variable names
 */
function extractVariablesFromComposite(
  compositeValue: CompositeObjectValue | undefined
): VariableSuggestion[] {
  if (!compositeValue || typeof compositeValue !== 'object') {
    return [];
  }

  const suggestions: VariableSuggestion[] = [];

  Object.entries(compositeValue).forEach(([varName, varValue]) => {
    if (!isCompositeValue(varValue)) {
      // If it's not a CompositeValue, treat it as a plain value
      let varType = 'any';
      let example = '';
      const plainValue = varValue as unknown;

      if (typeof plainValue === 'string') {
        varType = 'string';
        example =
          plainValue.length > 20 ? plainValue.slice(0, 20) + '...' : plainValue;
      } else if (typeof plainValue === 'number') {
        varType = Number.isInteger(plainValue) ? 'integer' : 'number';
        example = String(plainValue);
      } else if (typeof plainValue === 'boolean') {
        varType = 'boolean';
        example = String(plainValue);
      }

      suggestions.push({
        label: varName,
        value: varName,
        description: example || undefined,
        group: 'Variables',
        type: varType,
      });
      return;
    }

    // Determine the type based on the value
    let varType = 'any';
    let example = '';

    if (varValue.valueType === 'immediate') {
      const val = varValue.value;
      if (typeof val === 'string') {
        varType = 'string';
        example = val.length > 20 ? val.slice(0, 20) + '...' : val;
      } else if (typeof val === 'number') {
        varType = Number.isInteger(val) ? 'integer' : 'number';
        example = String(val);
      } else if (typeof val === 'boolean') {
        varType = 'boolean';
        example = String(val);
      }
    } else if (varValue.valueType === 'reference') {
      varType = 'reference';
      example = String(varValue.value);
    } else if (varValue.valueType === 'composite') {
      varType = Array.isArray(varValue.value) ? 'array' : 'object';
      example = varType === 'array' ? '[...]' : '{...}';
    }

    suggestions.push({
      label: varName,
      value: varName, // For template variables, just use the name directly
      description: example || undefined,
      group: 'Variables',
      type: varType,
    });
  });

  return suggestions;
}

/**
 * Get icon component based on variable type and path
 */
function getIconForType(type?: string, path?: string) {
  const lowerType = type?.toLowerCase() || '';
  const lowerPath = path?.toLowerCase() || '';

  if (lowerType.includes('string') || lowerType.includes('text')) {
    return <Icons.type className="size-3.5" />;
  }
  if (
    lowerType.includes('number') ||
    lowerType.includes('int') ||
    lowerType.includes('double') ||
    lowerType.includes('float')
  ) {
    return <Icons.hash className="size-3.5" />;
  }
  if (lowerType.includes('boolean') || lowerType.includes('bool')) {
    return <Icons.squareCheck className="size-3.5" />;
  }
  if (lowerType.includes('array') || lowerType.includes('list')) {
    return <Icons.list className="size-3.5" />;
  }
  if (lowerType.includes('object')) {
    return <Icons.braces className="size-3.5" />;
  }
  if (lowerType.includes('reference')) {
    return <Icons.gitBranch className="size-3.5" />;
  }
  if (
    lowerType.includes('date') ||
    lowerType.includes('time') ||
    lowerPath.includes('date') ||
    lowerPath.includes('time')
  ) {
    return <Icons.calendar className="size-3.5" />;
  }
  if (lowerPath.includes('email')) {
    return <Icons.mail className="size-3.5" />;
  }
  if (lowerPath.includes('name')) {
    return <Icons.user className="size-3.5" />;
  }

  return <Icons.variable className="size-3.5" />;
}

/**
 * Enhanced template editor modal with syntax highlighting, variable browser, and live preview
 */
export function TemplateEditorModal({
  open,
  onOpenChange,
  value,
  onChange,
  fieldName,
  placeholder = 'Enter your template here...\n\nUse {{ variable }} to insert variables\nUse {% if/for %} for control flow\nUse {# comment #} for comments',
}: TemplateEditorModalProps) {
  const [localValue, setLocalValue] = useState(value);
  const [viewMode, setViewMode] = useState<ViewMode>('editor');
  const [showVariables, setShowVariables] = useState(true);
  const [searchQuery, setSearchQuery] = useState('');
  const [copied, setCopied] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  // Get nodeId and available context variables from NodeFormContext
  const {
    nodeId,
    previousSteps,
    inputSchemaFields,
    variables,
    isInsideWhileLoop,
    isInsideSplit,
    isInsideWaitScope,
    splitItemSchemaFields,
  } = useContext(NodeFormContext);

  // Get the "context" field entry from the store for this node
  // The Render Template capability uses "context" to store template variables
  const contextEntry = useNodeFormStore((s) =>
    nodeId ? s.getFieldEntry(nodeId, 'context') : undefined
  );

  // Extract variables — use context field variables for Render Template capability,
  // otherwise fall back to all available reference variables (previous steps, inputs, workflow variables)
  const templateVariables = useMemo(() => {
    // First try context-specific variables (for Render Template capability)
    if (contextEntry) {
      if (
        contextEntry.valueType === 'composite' &&
        typeof contextEntry.value === 'object'
      ) {
        return extractVariablesFromComposite(
          contextEntry.value as CompositeObjectValue
        );
      }
      if (
        typeof contextEntry.value === 'object' &&
        contextEntry.value !== null &&
        !Array.isArray(contextEntry.value)
      ) {
        return extractVariablesFromPlainObject(
          contextEntry.value as Record<string, unknown>
        );
      }
    }

    // Fall back to all available reference variables (same as VariablePickerModal)
    return composeVariableSuggestions(
      previousSteps,
      inputSchemaFields,
      variables,
      isInsideWhileLoop,
      isInsideSplit,
      isInsideWaitScope,
      splitItemSchemaFields
    );
  }, [
    contextEntry,
    previousSteps,
    inputSchemaFields,
    variables,
    isInsideWhileLoop,
    isInsideSplit,
    isInsideWaitScope,
    splitItemSchemaFields,
  ]);

  // Filter variables by search query
  const filteredVariables = useMemo(() => {
    if (!searchQuery) return templateVariables;
    const lowerQuery = searchQuery.toLowerCase();
    return templateVariables.filter(
      (v) =>
        v.label.toLowerCase().includes(lowerQuery) ||
        v.description?.toLowerCase().includes(lowerQuery)
    );
  }, [templateVariables, searchQuery]);

  // Sync local value when modal opens
  useEffect(() => {
    if (open) {
      setLocalValue(value);
    }
  }, [open, value]);

  // Get template stats
  const stats = useMemo(() => getTemplateStats(localValue), [localValue]);

  // Generate preview
  const previewContent = useMemo(
    () => renderTemplatePreview(localValue, templateVariables),
    [localValue, templateVariables]
  );

  // Insert text at cursor position
  const insertAtCursor = (text: string) => {
    const textarea = textareaRef.current;
    if (!textarea) return;

    const start = textarea.selectionStart;
    const end = textarea.selectionEnd;

    const newValue = localValue.slice(0, start) + text + localValue.slice(end);
    setLocalValue(newValue);

    // Restore focus and cursor position
    setTimeout(() => {
      textarea.focus();
      const newPos = start + text.length;
      textarea.setSelectionRange(newPos, newPos);
    }, 0);
  };

  // Insert a variable reference
  const insertVariable = (variable: VariableSuggestion) => {
    insertAtCursor(`{{ ${variable.value} }}`);
  };

  // Insert a snippet
  const insertSnippet = (snippet: string) => {
    insertAtCursor(snippet);
  };

  // Copy to clipboard
  const copyToClipboard = () => {
    navigator.clipboard.writeText(localValue);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  // Handle save
  const handleSave = () => {
    onChange(localValue);
    onOpenChange(false);
  };

  // Handle cancel
  const handleCancel = () => {
    setLocalValue(value);
    onOpenChange(false);
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="flex max-h-[90vh] flex-col gap-0 overflow-hidden p-0 sm:max-w-4xl">
        {/* Header */}
        <DialogHeader className="shrink-0 border-b border-border bg-muted/30 px-6 py-4">
          <div className="flex items-center gap-3">
            <div className="flex size-9 items-center justify-center rounded-lg bg-primary/10">
              <Icons.code className="size-4 text-primary" />
            </div>
            <div>
              <DialogTitle className="text-base">Template Editor</DialogTitle>
              <DialogDescription className="text-xs">
                {fieldName
                  ? `Editing: ${fieldName}`
                  : 'Jinja2-style template with syntax highlighting'}
              </DialogDescription>
            </div>
          </div>
        </DialogHeader>

        {/* Toolbar */}
        <div className="flex shrink-0 items-center justify-between gap-2 border-b border-border bg-background px-4 py-2">
          {/* Snippets */}
          <div className="flex items-center gap-1">
            <span className="mr-1 text-xs text-muted-foreground">Insert:</span>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="h-7 px-2 font-mono text-xs text-purple-600 hover:bg-purple-50 dark:text-purple-400 dark:hover:bg-purple-950"
              onClick={() =>
                insertSnippet('{% if condition %}\n  \n{% endif %}')
              }
            >
              if
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="h-7 px-2 font-mono text-xs text-purple-600 hover:bg-purple-50 dark:text-purple-400 dark:hover:bg-purple-950"
              onClick={() =>
                insertSnippet(
                  '{% for item in items %}\n  {{ item }}\n{% endfor %}'
                )
              }
            >
              for
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="h-7 px-2 font-mono text-xs text-primary hover:bg-primary/10"
              onClick={() => insertSnippet('{{ value | default("") }}')}
            >
              default
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="h-7 px-2 font-mono text-xs text-muted-foreground hover:bg-muted"
              onClick={() => insertSnippet('{# comment #}')}
            >
              comment
            </Button>
          </div>

          {/* View mode tabs + variables toggle */}
          <div className="flex items-center gap-2">
            <Tabs
              value={viewMode}
              onValueChange={(v) => setViewMode(v as ViewMode)}
            >
              <TabsList className="h-8">
                <TabsTrigger value="editor" className="h-6 gap-1 px-2 text-xs">
                  <Icons.code className="size-3" />
                  Editor
                </TabsTrigger>
                <TabsTrigger value="preview" className="h-6 gap-1 px-2 text-xs">
                  <Icons.eye className="size-3" />
                  Preview
                </TabsTrigger>
                <TabsTrigger value="split" className="h-6 gap-1 px-2 text-xs">
                  <Icons.columns className="size-3" />
                  Split
                </TabsTrigger>
              </TabsList>
            </Tabs>

            <Button
              type="button"
              variant={showVariables ? 'secondary' : 'ghost'}
              size="sm"
              className="h-8 gap-1 px-2 text-xs"
              onClick={() => setShowVariables(!showVariables)}
            >
              <Icons.variable className="size-3.5" />
              Variables
            </Button>
          </div>
        </div>

        {/* Main content */}
        <div className="flex min-h-0 flex-1 overflow-hidden">
          {/* Editor / Preview area */}
          <div className="flex min-w-0 flex-1 flex-col">
            {(viewMode === 'editor' || viewMode === 'split') && (
              <div
                className={cn(
                  'flex flex-col',
                  viewMode === 'split'
                    ? 'h-1/2 border-b border-border'
                    : 'flex-1'
                )}
              >
                {/* Editor toolbar */}
                <div className="flex shrink-0 items-center justify-between border-b border-border bg-muted/30 px-3 py-1.5 text-xs">
                  <span className="font-medium text-muted-foreground">
                    Template
                  </span>
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    className="h-6 px-2 text-xs text-muted-foreground"
                    onClick={copyToClipboard}
                  >
                    {copied ? (
                      <>
                        <Icons.check className="mr-1 size-3" />
                        Copied!
                      </>
                    ) : (
                      <>
                        <Icons.copy className="mr-1 size-3" />
                        Copy
                      </>
                    )}
                  </Button>
                </div>
                {/* Textarea */}
                <div className="relative flex-1 overflow-hidden">
                  <textarea
                    ref={textareaRef}
                    value={localValue}
                    onChange={(e) => setLocalValue(e.target.value)}
                    className="absolute inset-0 h-full w-full resize-none bg-background p-3 font-mono text-sm text-foreground focus:outline-none focus-visible:ring-0"
                    placeholder={placeholder}
                    spellCheck={false}
                  />
                </div>
              </div>
            )}

            {(viewMode === 'preview' || viewMode === 'split') && (
              <div
                className={cn(
                  'flex flex-col',
                  viewMode === 'split' ? 'h-1/2' : 'flex-1'
                )}
              >
                {/* Preview header */}
                <div className="flex shrink-0 items-center gap-2 border-b border-green-100 bg-green-50 px-3 py-1.5 text-xs dark:border-green-900 dark:bg-green-950/30">
                  <Icons.eye className="size-3 text-green-600 dark:text-green-400" />
                  <span className="font-medium text-green-700 dark:text-green-400">
                    Preview with sample data
                  </span>
                </div>
                {/* Preview content */}
                <div className="flex-1 overflow-auto bg-muted/20 p-3">
                  <pre className="whitespace-pre-wrap font-mono text-sm text-foreground">
                    {previewContent || (
                      <span className="italic text-muted-foreground">
                        Empty template
                      </span>
                    )}
                  </pre>
                </div>
              </div>
            )}
          </div>

          {/* Variables panel */}
          {showVariables && (
            <div className="flex w-64 shrink-0 flex-col border-l border-border bg-muted/20">
              <div className="shrink-0 border-b border-border p-3">
                <h3 className="mb-1 flex items-center gap-1.5 text-xs font-semibold text-foreground">
                  <Icons.variable className="size-3.5" />
                  Template Variables
                </h3>
                <p className="text-2xs text-muted-foreground">
                  {templateVariables.length > 0
                    ? 'Click to insert at cursor'
                    : 'Define variables in the "variables" field'}
                </p>
                {/* Search - only show if there are variables */}
                {templateVariables.length > 0 && (
                  <div className="relative mt-2">
                    <Icons.search className="absolute left-2 top-1/2 size-3 -translate-y-1/2 text-muted-foreground" />
                    <input
                      type="text"
                      placeholder="Search..."
                      value={searchQuery}
                      onChange={(e) => setSearchQuery(e.target.value)}
                      className="h-7 w-full rounded border border-input bg-background pl-7 pr-2 text-xs focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                    />
                  </div>
                )}
              </div>

              <div className="flex-1 overflow-auto p-2">
                {filteredVariables.length === 0 ? (
                  <div className="py-6 text-center text-xs text-muted-foreground">
                    {templateVariables.length === 0 ? (
                      <div className="space-y-2">
                        <Icons.inbox className="mx-auto size-8 opacity-50" />
                        <p>No variables defined</p>
                        <p className="text-2xs">
                          Add variables using the
                          <br />
                          "variables" field above
                        </p>
                      </div>
                    ) : (
                      'No matching variables'
                    )}
                  </div>
                ) : (
                  <div className="space-y-0.5">
                    {filteredVariables.map((variable) => (
                      <button
                        key={variable.value}
                        type="button"
                        onClick={() => insertVariable(variable)}
                        className="group w-full rounded border border-transparent p-2 text-left transition-colors hover:border-primary/30 hover:bg-primary/5"
                      >
                        <div className="flex items-center gap-1.5">
                          <span className="text-muted-foreground group-hover:text-primary">
                            {getIconForType(variable.type, variable.value)}
                          </span>
                          <code className="truncate text-xs font-semibold text-primary">
                            {variable.label}
                          </code>
                          {variable.type && (
                            <span className="ml-auto text-3xs text-muted-foreground">
                              {variable.type}
                            </span>
                          )}
                        </div>
                        {variable.description && (
                          <div className="mt-0.5 truncate pl-5 text-3xs text-muted-foreground">
                            {variable.description}
                          </div>
                        )}
                      </button>
                    ))}
                  </div>
                )}
              </div>
            </div>
          )}
        </div>

        {/* Footer */}
        <div className="flex shrink-0 items-center justify-between border-t border-border bg-muted/30 px-4 py-3">
          <div className="text-xs text-muted-foreground">
            {stats.characters} characters
            {stats.variables > 0 && <span className="mx-1">•</span>}
            {stats.variables > 0 &&
              `${stats.variables} variable${stats.variables !== 1 ? 's' : ''}`}
            {stats.controls > 0 && <span className="mx-1">•</span>}
            {stats.controls > 0 &&
              `${stats.controls} control${stats.controls !== 1 ? 's' : ''}`}
          </div>
          <div className="flex items-center gap-2">
            <Button type="button" variant="ghost" onClick={handleCancel}>
              Cancel
            </Button>
            <Button type="button" onClick={handleSave}>
              Save Template
            </Button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
