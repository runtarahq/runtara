import { describe, it, expect } from 'vitest';

/**
 * Tests for inputMapping value preservation through save/load cycle.
 *
 * Issue: Values entered in input mapping fields (especially integers)
 * are not being preserved after save and reload.
 *
 * The flow is:
 * 1. User enters value "5" in an integer field
 * 2. SimpleInputMappingEditor stores { type: 'fieldName', value: '5', valueType: 'immediate', typeHint: 'integer' }
 * 3. On save, cleanNodeData -> processMappingEntry converts to backend format
 * 4. Backend stores and returns the value
 * 5. On load, executionGraphToReactFlow converts back to UI format
 * 6. SimpleInputMappingEditor should display "5"
 */

// Simulate the processMappingEntry logic from utils.tsx
// Uses API ValueType convention directly (no internal name mapping)
function processMappingEntry({
  type,
  value,
  typeHint,
  valueType,
}: {
  type: string;
  value: any;
  typeHint?: string;
  valueType?: 'reference' | 'immediate';
}) {
  let finalValue = value;

  if (typeof value === 'string' && value) {
    const isTemplate = value.includes('{{');

    if (!isTemplate) {
      // JSON parsing - only when typeHint is explicitly 'json'
      // No auto-detection based on string content
      if (typeHint === 'json') {
        try {
          finalValue = JSON.parse(value);
        } catch {
          finalValue = value;
        }
      }

      // Convert numeric strings to actual numbers for integer/number type hints
      if (typeHint === 'integer' || typeHint === 'number') {
        const numValue = Number(value);
        if (!isNaN(numValue)) {
          finalValue = typeHint === 'integer' ? Math.trunc(numValue) : numValue;
        }
      }

      // Convert boolean strings to actual booleans for boolean type hint
      if (typeHint === 'boolean') {
        const lowerValue = value.toLowerCase();
        if (lowerValue === 'true' || lowerValue === '1') {
          finalValue = true;
        } else if (lowerValue === 'false' || lowerValue === '0') {
          finalValue = false;
        }
      }
    }
  }

  const resolvedValueType: 'reference' | 'immediate' =
    valueType ||
    (typeof finalValue === 'string' && finalValue.includes('{{')
      ? 'reference'
      : 'immediate');

  const mappingValue: {
    valueType: 'reference' | 'immediate';
    value: any;
    type?: string;
  } = {
    valueType: resolvedValueType,
    value: finalValue,
  };

  if (typeHint && typeHint !== 'auto') {
    mappingValue.type = typeHint;
  }

  return [type, mappingValue];
}

// Simulate the reverse conversion from executionGraphToReactFlow
function convertBackendToUI(fieldName: string, backendValue: any) {
  if (
    typeof backendValue === 'object' &&
    backendValue !== null &&
    'value' in backendValue
  ) {
    return {
      type: fieldName,
      value: backendValue.value,
      typeHint: backendValue.type || 'auto',
      valueType: backendValue.valueType || 'immediate',
    };
  }

  // Legacy format
  return {
    type: fieldName,
    value: backendValue,
    typeHint: 'auto',
    valueType: 'immediate' as const,
  };
}

// Helper type for the backend value format
type BackendMappingValue = { valueType: string; value: any; type?: string };

describe('InputMapping Value Preservation', () => {
  describe('processMappingEntry - converting UI values to backend format', () => {
    it('should convert string "5" to number 5 when typeHint is "integer"', () => {
      const res = processMappingEntry({
        type: 'count',
        value: '5',
        typeHint: 'integer',
        valueType: 'immediate',
      });
      const fieldName = res[0] as string;
      const result = res[1] as BackendMappingValue;

      expect(fieldName).toBe('count');
      expect(result.value).toBe(5);
      expect(typeof result.value).toBe('number');
      expect(result.type).toBe('integer');
    });

    it('should convert string "3.14" to number 3.14 when typeHint is "number"', () => {
      const res = processMappingEntry({
        type: 'price',
        value: '3.14',
        typeHint: 'number',
        valueType: 'immediate',
      });
      const result = res[1] as BackendMappingValue;

      expect(result.value).toBe(3.14);
      expect(typeof result.value).toBe('number');
      expect(result.type).toBe('number');
    });

    it('should convert string "true" to boolean true when typeHint is "boolean"', () => {
      const res = processMappingEntry({
        type: 'enabled',
        value: 'true',
        typeHint: 'boolean',
        valueType: 'immediate',
      });
      const result = res[1] as BackendMappingValue;

      expect(result.value).toBe(true);
      expect(typeof result.value).toBe('boolean');
    });

    it('should keep string as-is when typeHint is "string"', () => {
      const res = processMappingEntry({
        type: 'name',
        value: 'hello',
        typeHint: 'string',
        valueType: 'immediate',
      });
      const result = res[1] as { value: any };

      expect(result.value).toBe('hello');
      expect(typeof result.value).toBe('string');
    });

    it('should NOT convert when typeHint is undefined', () => {
      const res = processMappingEntry({
        type: 'unknown',
        value: '5',
        typeHint: undefined,
        valueType: 'immediate',
      });
      const result = res[1] as { value: any };

      expect(result.value).toBe('5');
      expect(typeof result.value).toBe('string');
    });

    it('should NOT convert when typeHint is "auto"', () => {
      const res = processMappingEntry({
        type: 'unknown',
        value: '5',
        typeHint: 'auto',
        valueType: 'immediate',
      });
      const result = res[1] as { value: any };

      expect(result.value).toBe('5');
      expect(typeof result.value).toBe('string');
    });
  });

  describe('Round-trip conversion (UI -> Backend -> UI)', () => {
    it('should preserve integer value through save/load cycle', () => {
      // Step 1: UI has this entry
      const uiEntry = {
        type: 'count',
        value: '5',
        typeHint: 'integer',
        valueType: 'immediate' as const,
      };

      // Step 2: Convert to backend format
      const result = processMappingEntry(uiEntry);
      const fieldName = result[0] as string;
      const backendValue = result[1] as {
        valueType: string;
        value: any;
        type?: string;
      };

      // Step 3: Simulate what backend returns
      const backendData: Record<string, any> = { [fieldName]: backendValue };

      // Step 4: Convert back to UI format
      const loadedEntry = convertBackendToUI(fieldName, backendData[fieldName]);

      // Step 5: Verify the value is preserved
      expect(loadedEntry.value).toBe(5); // Now a number
      expect(loadedEntry.typeHint).toBe('integer');

      // When displayed in input, String(5) = "5"
      expect(String(loadedEntry.value)).toBe('5');
    });

    it('should preserve number value through save/load cycle', () => {
      const uiEntry = {
        type: 'price',
        value: '19.99',
        typeHint: 'number',
        valueType: 'immediate' as const,
      };

      const result = processMappingEntry(uiEntry);
      const fieldName = result[0] as string;
      const backendValue = result[1] as {
        valueType: string;
        value: any;
        type?: string;
      };
      const loadedEntry = convertBackendToUI(fieldName, backendValue);

      expect(loadedEntry.value).toBe(19.99);
      expect(String(loadedEntry.value)).toBe('19.99');
    });

    it('should preserve boolean value through save/load cycle', () => {
      const uiEntry = {
        type: 'enabled',
        value: 'true',
        typeHint: 'boolean',
        valueType: 'immediate' as const,
      };

      const result = processMappingEntry(uiEntry);
      const fieldName = result[0] as string;
      const backendValue = result[1] as {
        valueType: string;
        value: any;
        type?: string;
      };
      const loadedEntry = convertBackendToUI(fieldName, backendValue);

      expect(loadedEntry.value).toBe(true);
    });
  });

  describe('Filter behavior with empty values', () => {
    it('should filter out empty string values', () => {
      const entries = [
        {
          type: 'field1',
          value: '',
          typeHint: 'integer',
          valueType: 'immediate',
        },
        {
          type: 'field2',
          value: '5',
          typeHint: 'integer',
          valueType: 'immediate',
        },
        {
          type: 'field3',
          value: null,
          typeHint: 'string',
          valueType: 'immediate',
        },
      ];

      const filtered = entries.filter(({ value }) => {
        if (value === undefined || value === null || value === '') {
          return false;
        }
        return true;
      });

      expect(filtered).toHaveLength(1);
      expect(filtered[0].type).toBe('field2');
    });

    it('should NOT filter out numeric zero', () => {
      const entries = [
        {
          type: 'field1',
          value: 0,
          typeHint: 'integer',
          valueType: 'immediate',
        },
        {
          type: 'field2',
          value: '0',
          typeHint: 'integer',
          valueType: 'immediate',
        },
      ];

      const filtered = entries.filter(({ value }) => {
        if (value === undefined || value === null || value === '') {
          return false;
        }
        return true;
      });

      expect(filtered).toHaveLength(2);
    });
  });

  describe('TypeHint flow verification', () => {
    it('should fail if typeHint is lost during data flow', () => {
      // This test documents the bug: if typeHint is dropped, conversion doesn't happen

      // Simulate what happens when typeHint is NOT included in the data flow
      const entryWithoutTypeHint = {
        type: 'count',
        value: '5',
        valueType: 'immediate' as const,
        // typeHint is MISSING
      };

      const result = processMappingEntry(entryWithoutTypeHint);
      const backendValue = result[1] as {
        valueType: string;
        value: any;
        type?: string;
      };

      // BUG: Without typeHint, the value remains a string!
      expect(backendValue.value).toBe('5');
      expect(typeof backendValue.value).toBe('string'); // This causes the type incompatibility error
    });

    it('should succeed when typeHint is properly included', () => {
      const entryWithTypeHint = {
        type: 'count',
        value: '5',
        valueType: 'immediate' as const,
        typeHint: 'integer',
      };

      const result = processMappingEntry(entryWithTypeHint);
      const backendValue = result[1] as {
        valueType: string;
        value: any;
        type?: string;
      };

      // With typeHint, the value is properly converted
      expect(backendValue.value).toBe(5);
      expect(typeof backendValue.value).toBe('number');
    });
  });

  describe('JSON auto-detection removal', () => {
    it('should NOT parse JSON-looking string when typeHint is undefined', () => {
      const slackBlocks =
        '{"blocks":[{"type":"header","text":{"type":"plain_text","text":"Hello"}}]}';
      const res = processMappingEntry({
        type: 'text',
        value: slackBlocks,
        typeHint: undefined,
        valueType: 'immediate',
      });
      const result = res[1] as BackendMappingValue;

      // Value should remain a string, NOT be parsed into an object
      expect(typeof result.value).toBe('string');
      expect(result.value).toBe(slackBlocks);
    });

    it('should NOT parse JSON-looking string when typeHint is "auto"', () => {
      const jsonArray = '[1, 2, 3]';
      const res = processMappingEntry({
        type: 'items',
        value: jsonArray,
        typeHint: 'auto',
        valueType: 'immediate',
      });
      const result = res[1] as BackendMappingValue;

      // Value should remain a string, NOT be parsed into an array
      expect(typeof result.value).toBe('string');
      expect(result.value).toBe(jsonArray);
    });

    it('should NOT parse JSON object string when typeHint is "string"', () => {
      const jsonObject = '{"key": "value"}';
      const res = processMappingEntry({
        type: 'template',
        value: jsonObject,
        typeHint: 'string',
        valueType: 'immediate',
      });
      const result = res[1] as BackendMappingValue;

      expect(typeof result.value).toBe('string');
      expect(result.value).toBe(jsonObject);
    });

    it('should parse JSON string ONLY when typeHint is explicitly "json"', () => {
      const jsonObject = '{"key": "value"}';
      const res = processMappingEntry({
        type: 'data',
        value: jsonObject,
        typeHint: 'json',
        valueType: 'immediate',
      });
      const result = res[1] as BackendMappingValue;

      // Only with explicit 'json' typeHint should parsing occur
      expect(typeof result.value).toBe('object');
      expect(result.value).toEqual({ key: 'value' });
    });

    it('should parse JSON array ONLY when typeHint is explicitly "json"', () => {
      const jsonArray = '[1, 2, 3]';
      const res = processMappingEntry({
        type: 'items',
        value: jsonArray,
        typeHint: 'json',
        valueType: 'immediate',
      });
      const result = res[1] as BackendMappingValue;

      expect(Array.isArray(result.value)).toBe(true);
      expect(result.value).toEqual([1, 2, 3]);
    });

    it('should keep invalid JSON as string even when typeHint is "json"', () => {
      const invalidJson = '{invalid json}';
      const res = processMappingEntry({
        type: 'data',
        value: invalidJson,
        typeHint: 'json',
        valueType: 'immediate',
      });
      const result = res[1] as BackendMappingValue;

      // Invalid JSON should remain as string
      expect(typeof result.value).toBe('string');
      expect(result.value).toBe(invalidJson);
    });

    it('should preserve Slack blocks as string for render-template capability', () => {
      // This is the exact workflow that was causing the bug
      const slackMessage = JSON.stringify({
        blocks: [
          {
            type: 'header',
            text: {
              type: 'plain_text',
              text: 'Travion Catalog Sync - COMPLETED',
            },
          },
          {
            type: 'section',
            text: { type: 'mrkdwn', text: '*Status:* No files found' },
          },
        ],
      });

      const res = processMappingEntry({
        type: 'text',
        value: slackMessage,
        typeHint: undefined, // render-template text field has no typeHint
        valueType: 'immediate',
      });
      const result = res[1] as BackendMappingValue;

      // Must remain a string for the backend to accept it
      expect(typeof result.value).toBe('string');
      expect(result.value).toBe(slackMessage);
    });
  });
});
