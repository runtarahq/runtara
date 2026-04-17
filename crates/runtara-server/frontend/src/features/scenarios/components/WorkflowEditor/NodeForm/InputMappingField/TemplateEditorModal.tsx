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
} from '@/features/scenarios/stores/nodeFormStore';
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
    return <Icons.type className="h-3.5 w-3.5" />;
  }
  if (
    lowerType.includes('number') ||
    lowerType.includes('int') ||
    lowerType.includes('double') ||
    lowerType.includes('float')
  ) {
    return <Icons.hash className="h-3.5 w-3.5" />;
  }
  if (lowerType.includes('boolean') || lowerType.includes('bool')) {
    return <Icons.squareCheck className="h-3.5 w-3.5" />;
  }
  if (lowerType.includes('array') || lowerType.includes('list')) {
    return <Icons.list className="h-3.5 w-3.5" />;
  }
  if (lowerType.includes('object')) {
    return <Icons.braces className="h-3.5 w-3.5" />;
  }
  if (lowerType.includes('reference')) {
    return <Icons.gitBranch className="h-3.5 w-3.5" />;
  }
  if (
    lowerType.includes('date') ||
    lowerType.includes('time') ||
    lowerPath.includes('date') ||
    lowerPath.includes('time')
  ) {
    return <Icons.calendar className="h-3.5 w-3.5" />;
  }
  if (lowerPath.includes('email')) {
    return <Icons.mail className="h-3.5 w-3.5" />;
  }
  if (lowerPath.includes('name')) {
    return <Icons.user className="h-3.5 w-3.5" />;
  }

  return <Icons.variable className="h-3.5 w-3.5" />;
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
  const { nodeId, previousSteps, inputSchemaFields, variables } =
    useContext(NodeFormContext);

  // Get the "context" field entry from the store for this node
  // The Render Template capability uses "context" to store template variables
  const contextEntry = useNodeFormStore((s) =>
    nodeId ? s.getFieldEntry(nodeId, 'context') : undefined
  );

  // Extract variables — use context field variables for Render Template capability,
  // otherwise fall back to all available reference variables (previous steps, inputs, scenario variables)
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
      variables
    );
  }, [contextEntry, previousSteps, inputSchemaFields, variables]);

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
      <DialogContent className="sm:max-w-4xl max-h-[90vh] flex flex-col p-0 gap-0 overflow-hidden">
        {/* Header */}
        <DialogHeader className="px-6 py-4 border-b border-border bg-muted/30 shrink-0">
          <div className="flex items-center gap-3">
            <div className="w-9 h-9 rounded-lg bg-primary/10 flex items-center justify-center">
              <Icons.code className="w-4 h-4 text-primary" />
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
        <div className="flex items-center justify-between gap-2 px-4 py-2 border-b border-border bg-background shrink-0">
          {/* Snippets */}
          <div className="flex items-center gap-1">
            <span className="text-xs text-muted-foreground mr-1">Insert:</span>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="h-7 px-2 font-mono text-xs text-purple-600 dark:text-purple-400 hover:bg-purple-50 dark:hover:bg-purple-950"
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
              className="h-7 px-2 font-mono text-xs text-purple-600 dark:text-purple-400 hover:bg-purple-50 dark:hover:bg-purple-950"
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
              className="h-7 px-2 font-mono text-xs text-blue-600 dark:text-blue-400 hover:bg-blue-50 dark:hover:bg-blue-950"
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
                <TabsTrigger value="editor" className="h-6 px-2 text-xs gap-1">
                  <Icons.code className="h-3 w-3" />
                  Editor
                </TabsTrigger>
                <TabsTrigger value="preview" className="h-6 px-2 text-xs gap-1">
                  <Icons.eye className="h-3 w-3" />
                  Preview
                </TabsTrigger>
                <TabsTrigger value="split" className="h-6 px-2 text-xs gap-1">
                  <Icons.columns className="h-3 w-3" />
                  Split
                </TabsTrigger>
              </TabsList>
            </Tabs>

            <Button
              type="button"
              variant={showVariables ? 'secondary' : 'ghost'}
              size="sm"
              className="h-8 px-2 text-xs gap-1"
              onClick={() => setShowVariables(!showVariables)}
            >
              <Icons.variable className="h-3.5 w-3.5" />
              Variables
            </Button>
          </div>
        </div>

        {/* Main content */}
        <div className="flex-1 flex min-h-0 overflow-hidden">
          {/* Editor / Preview area */}
          <div className="flex-1 flex flex-col min-w-0">
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
                <div className="flex items-center justify-between px-3 py-1.5 bg-muted/30 border-b border-border text-xs shrink-0">
                  <span className="text-muted-foreground font-medium">
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
                        <Icons.check className="h-3 w-3 mr-1" />
                        Copied!
                      </>
                    ) : (
                      <>
                        <Icons.copy className="h-3 w-3 mr-1" />
                        Copy
                      </>
                    )}
                  </Button>
                </div>
                {/* Textarea */}
                <div className="flex-1 relative overflow-hidden">
                  <textarea
                    ref={textareaRef}
                    value={localValue}
                    onChange={(e) => setLocalValue(e.target.value)}
                    className="absolute inset-0 w-full h-full p-3 font-mono text-sm resize-none focus:outline-none focus:ring-0 bg-background text-foreground"
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
                <div className="flex items-center gap-2 px-3 py-1.5 bg-green-50 dark:bg-green-950/30 border-b border-green-100 dark:border-green-900 text-xs shrink-0">
                  <Icons.eye className="h-3 w-3 text-green-600 dark:text-green-400" />
                  <span className="text-green-700 dark:text-green-400 font-medium">
                    Preview with sample data
                  </span>
                </div>
                {/* Preview content */}
                <div className="flex-1 p-3 bg-muted/20 overflow-auto">
                  <pre className="font-mono text-sm text-foreground whitespace-pre-wrap">
                    {previewContent || (
                      <span className="text-muted-foreground italic">
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
            <div className="w-64 border-l border-border bg-muted/20 flex flex-col shrink-0">
              <div className="p-3 border-b border-border shrink-0">
                <h3 className="text-xs font-semibold text-foreground flex items-center gap-1.5 mb-1">
                  <Icons.variable className="h-3.5 w-3.5" />
                  Template Variables
                </h3>
                <p className="text-[11px] text-muted-foreground">
                  {templateVariables.length > 0
                    ? 'Click to insert at cursor'
                    : 'Define variables in the "variables" field'}
                </p>
                {/* Search - only show if there are variables */}
                {templateVariables.length > 0 && (
                  <div className="relative mt-2">
                    <Icons.search className="absolute left-2 top-1/2 -translate-y-1/2 h-3 w-3 text-muted-foreground" />
                    <input
                      type="text"
                      placeholder="Search..."
                      value={searchQuery}
                      onChange={(e) => setSearchQuery(e.target.value)}
                      className="w-full h-7 pl-7 pr-2 text-xs rounded border border-input bg-background focus:outline-none focus:ring-1 focus:ring-ring"
                    />
                  </div>
                )}
              </div>

              <div className="flex-1 overflow-auto p-2">
                {filteredVariables.length === 0 ? (
                  <div className="text-center py-6 text-muted-foreground text-xs">
                    {templateVariables.length === 0 ? (
                      <div className="space-y-2">
                        <Icons.inbox className="h-8 w-8 mx-auto opacity-50" />
                        <p>No variables defined</p>
                        <p className="text-[11px]">
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
                        className="w-full text-left p-2 rounded border border-transparent hover:border-primary/30 hover:bg-primary/5 transition-colors group"
                      >
                        <div className="flex items-center gap-1.5">
                          <span className="text-muted-foreground group-hover:text-primary">
                            {getIconForType(variable.type, variable.value)}
                          </span>
                          <code className="text-xs font-semibold text-primary truncate">
                            {variable.label}
                          </code>
                          {variable.type && (
                            <span className="text-[10px] text-muted-foreground ml-auto">
                              {variable.type}
                            </span>
                          )}
                        </div>
                        {variable.description && (
                          <div className="text-[10px] text-muted-foreground mt-0.5 pl-5 truncate">
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
        <div className="flex items-center justify-between px-4 py-3 border-t border-border bg-muted/30 shrink-0">
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
