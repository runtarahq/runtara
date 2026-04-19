import { VariableSuggestion } from '../InputMappingValueField/VariableSuggestions';

/**
 * Sample data for different variable types used in preview
 */
const SAMPLE_VALUES: Record<string, string> = {
  string: 'Sample Text',
  str: 'Sample Text',
  text: 'Sample Text',
  number: '42',
  integer: '100',
  int: '100',
  float: '3.14',
  double: '3.14159',
  boolean: 'true',
  bool: 'true',
  date: '2026-01-25',
  datetime: '2026-01-25T10:30:00Z',
  time: '10:30:00',
  email: 'user@example.com',
  url: 'https://example.com',
  array: '[item1, item2, item3]',
  object: '{...}',
  any: 'value',
};

/**
 * Gets a sample value for a variable based on its type or name
 */
function getSampleValue(variable: VariableSuggestion): string {
  // First try exact type match
  if (variable.type) {
    const lowerType = variable.type.toLowerCase();
    if (SAMPLE_VALUES[lowerType]) {
      return SAMPLE_VALUES[lowerType];
    }
  }

  // Infer from variable name
  const lowerName = variable.label.toLowerCase();

  if (lowerName.includes('email')) return 'user@example.com';
  if (lowerName.includes('name')) return 'John Doe';
  if (lowerName.includes('id') || lowerName.includes('key')) return 'abc-123';
  if (
    lowerName.includes('price') ||
    lowerName.includes('amount') ||
    lowerName.includes('total') ||
    lowerName.includes('cost')
  )
    return '99.99';
  if (
    lowerName.includes('count') ||
    lowerName.includes('quantity') ||
    lowerName.includes('qty')
  )
    return '5';
  if (lowerName.includes('date')) return '2026-01-25';
  if (lowerName.includes('time')) return '10:30:00';
  if (lowerName.includes('url') || lowerName.includes('link'))
    return 'https://example.com';
  if (lowerName.includes('phone')) return '+1 (555) 123-4567';
  if (lowerName.includes('address')) return '123 Main St, City';
  if (
    lowerName.includes('description') ||
    lowerName.includes('text') ||
    lowerName.includes('content')
  )
    return 'Sample description text';
  if (lowerName.includes('items') || lowerName.includes('list')) return '[...]';

  return variable.type || 'value';
}

/**
 * Generates sample data map from available variables
 */
function generateSampleData(
  variables: VariableSuggestion[]
): Record<string, string> {
  const data: Record<string, string> = {};

  for (const variable of variables) {
    // Use the full value path as key (e.g., "workflow.inputs.data.name")
    data[variable.value] = getSampleValue(variable);
    // Also add just the label for simple lookups
    data[variable.label] = getSampleValue(variable);
  }

  return data;
}

/**
 * Renders a template preview by replacing variables with sample data
 * This is a simple string replacement - not a full Jinja2 implementation
 */
export function renderTemplatePreview(
  template: string,
  variables: VariableSuggestion[]
): string {
  if (!template) return '';

  let preview = template;
  const sampleData = generateSampleData(variables);

  // Replace {{ variable }} patterns with sample values
  preview = preview.replace(
    /\{\{\s*([^}|]+?)(?:\s*\|[^}]*)?\s*\}\}/g,
    (_match, varName) => {
      const trimmedVar = varName.trim();

      // Try to find a matching variable
      if (sampleData[trimmedVar]) {
        return sampleData[trimmedVar];
      }

      // Try partial match (just the last part of the path)
      const lastPart = trimmedVar.split('.').pop() || '';
      if (sampleData[lastPart]) {
        return sampleData[lastPart];
      }

      // Default: show the variable name as placeholder
      return `[${trimmedVar}]`;
    }
  );

  // Remove {% control %} blocks for preview (simplified)
  preview = preview.replace(/\{%\s*if\s+[^%]*%\}/g, '');
  preview = preview.replace(/\{%\s*endif\s*%\}/g, '');
  preview = preview.replace(/\{%\s*else\s*%\}/g, '');
  preview = preview.replace(/\{%\s*elif\s+[^%]*%\}/g, '');

  // Handle for loops - just show placeholder
  preview = preview.replace(
    /\{%\s*for\s+(\w+)\s+in\s+[^%]*%\}([\s\S]*?)\{%\s*endfor\s*%\}/g,
    (_match, itemVar, content) => {
      // Show the content once with sample item
      return content.replace(
        new RegExp(`\\{\\{\\s*${itemVar}(?:\\.[^}]*)?\\s*\\}\\}`, 'g'),
        '[item]'
      );
    }
  );

  // Remove {# comment #} blocks
  preview = preview.replace(/\{#[\s\S]*?#\}/g, '');

  // Clean up extra whitespace
  preview = preview.replace(/\n{3,}/g, '\n\n');

  return preview.trim();
}

/**
 * Counts template statistics
 */
export function getTemplateStats(template: string): {
  characters: number;
  variables: number;
  controls: number;
  comments: number;
} {
  if (!template) {
    return { characters: 0, variables: 0, controls: 0, comments: 0 };
  }

  const variableMatches = template.match(/\{\{[\s\S]*?\}\}/g);
  const controlMatches = template.match(/\{%[\s\S]*?%\}/g);
  const commentMatches = template.match(/\{#[\s\S]*?#\}/g);

  return {
    characters: template.length,
    variables: variableMatches ? variableMatches.length : 0,
    controls: controlMatches ? controlMatches.length : 0,
    comments: commentMatches ? commentMatches.length : 0,
  };
}
