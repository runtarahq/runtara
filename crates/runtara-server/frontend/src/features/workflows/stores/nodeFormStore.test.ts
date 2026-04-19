import { describe, expect, it, beforeEach } from 'vitest';
import { useNodeFormStore, InputMappingEntry } from './nodeFormStore';

describe('nodeFormStore', () => {
  beforeEach(() => {
    // Reset store state before each test
    useNodeFormStore.setState({ nodeInputMappings: {} });
  });

  describe('setFieldValue', () => {
    it('sets a field value for a node', () => {
      useNodeFormStore.getState().setFieldValue('node-1', 'fieldA', 'value1');

      const entry = useNodeFormStore
        .getState()
        .getFieldEntry('node-1', 'fieldA');
      expect(entry?.value).toBe('value1');
      expect(entry?.type).toBe('fieldA');
      expect(entry?.valueType).toBe('immediate');
    });

    it('creates node mapping if it does not exist', () => {
      useNodeFormStore.getState().setFieldValue('new-node', 'field', 'test');

      const state = useNodeFormStore.getState();
      expect(state.nodeInputMappings['new-node']).toBeDefined();
    });

    it('updates existing field value', () => {
      useNodeFormStore.getState().setFieldValue('node-1', 'field', 'initial');
      useNodeFormStore.getState().setFieldValue('node-1', 'field', 'updated');

      const entry = useNodeFormStore
        .getState()
        .getFieldEntry('node-1', 'field');
      expect(entry?.value).toBe('updated');
    });
  });

  describe('setFieldValueType', () => {
    it('sets value type to reference', () => {
      useNodeFormStore.getState().setFieldValue('node-1', 'field', 'test');
      useNodeFormStore
        .getState()
        .setFieldValueType('node-1', 'field', 'reference');

      const entry = useNodeFormStore
        .getState()
        .getFieldEntry('node-1', 'field');
      expect(entry?.valueType).toBe('reference');
    });

    it('creates field if it does not exist', () => {
      useNodeFormStore
        .getState()
        .setFieldValueType('node-1', 'newField', 'reference');

      const entry = useNodeFormStore
        .getState()
        .getFieldEntry('node-1', 'newField');
      expect(entry?.valueType).toBe('reference');
      expect(entry?.value).toBe('');
    });
  });

  describe('setFieldTypeHint', () => {
    it('sets type hint for a field', () => {
      useNodeFormStore.getState().setFieldValue('node-1', 'field', '123');
      useNodeFormStore
        .getState()
        .setFieldTypeHint('node-1', 'field', 'integer');

      const entry = useNodeFormStore
        .getState()
        .getFieldEntry('node-1', 'field');
      expect(entry?.typeHint).toBe('integer');
    });
  });

  describe('initializeField', () => {
    it('initializes a new field', () => {
      const entry: InputMappingEntry = {
        type: 'testField',
        value: 'testValue',
        valueType: 'immediate',
        typeHint: 'string',
      };

      useNodeFormStore.getState().initializeField('node-1', 'testField', entry);

      const result = useNodeFormStore
        .getState()
        .getFieldEntry('node-1', 'testField');
      expect(result).toEqual(entry);
    });

    it('does not overwrite existing field', () => {
      const original: InputMappingEntry = {
        type: 'field',
        value: 'original',
        valueType: 'immediate',
      };
      const newEntry: InputMappingEntry = {
        type: 'field',
        value: 'new',
        valueType: 'reference',
      };

      useNodeFormStore.getState().initializeField('node-1', 'field', original);
      useNodeFormStore.getState().initializeField('node-1', 'field', newEntry);

      const result = useNodeFormStore
        .getState()
        .getFieldEntry('node-1', 'field');
      expect(result?.value).toBe('original');
    });
  });

  describe('removeField', () => {
    it('removes a field from a node', () => {
      useNodeFormStore.getState().setFieldValue('node-1', 'fieldA', 'value');
      useNodeFormStore.getState().setFieldValue('node-1', 'fieldB', 'value');

      useNodeFormStore.getState().removeField('node-1', 'fieldA');

      expect(
        useNodeFormStore.getState().getFieldEntry('node-1', 'fieldA')
      ).toBeUndefined();
      expect(
        useNodeFormStore.getState().getFieldEntry('node-1', 'fieldB')
      ).toBeDefined();
    });

    it('handles removing from non-existent node gracefully', () => {
      expect(() => {
        useNodeFormStore.getState().removeField('non-existent', 'field');
      }).not.toThrow();
    });
  });

  describe('getNodeInputMapping', () => {
    it('returns all field entries for a node', () => {
      useNodeFormStore.getState().setFieldValue('node-1', 'field1', 'value1');
      useNodeFormStore.getState().setFieldValue('node-1', 'field2', 'value2');

      const mapping = useNodeFormStore.getState().getNodeInputMapping('node-1');

      expect(mapping).toHaveLength(2);
      expect(mapping.map((e) => e.type)).toContain('field1');
      expect(mapping.map((e) => e.type)).toContain('field2');
    });

    it('returns empty array for non-existent node', () => {
      const mapping = useNodeFormStore
        .getState()
        .getNodeInputMapping('non-existent');
      expect(mapping).toEqual([]);
    });
  });

  describe('clearNode', () => {
    it('removes all data for a node', () => {
      useNodeFormStore.getState().setFieldValue('node-1', 'field1', 'value1');
      useNodeFormStore.getState().setFieldValue('node-1', 'field2', 'value2');

      useNodeFormStore.getState().clearNode('node-1');

      expect(useNodeFormStore.getState().getNodeInputMapping('node-1')).toEqual(
        []
      );
    });
  });

  describe('loadNodeData', () => {
    it('loads multiple entries for a node', () => {
      const entries: InputMappingEntry[] = [
        { type: 'field1', value: 'value1', valueType: 'immediate' },
        {
          type: 'field2',
          value: 'value2',
          valueType: 'reference',
          typeHint: 'json',
        },
      ];

      useNodeFormStore.getState().loadNodeData('node-1', entries);

      const mapping = useNodeFormStore.getState().getNodeInputMapping('node-1');
      expect(mapping).toHaveLength(2);
      expect(
        useNodeFormStore.getState().getFieldEntry('node-1', 'field1')?.value
      ).toBe('value1');
      expect(
        useNodeFormStore.getState().getFieldEntry('node-1', 'field2')?.typeHint
      ).toBe('json');
    });

    it('replaces existing data for a node', () => {
      useNodeFormStore
        .getState()
        .setFieldValue('node-1', 'oldField', 'oldValue');

      useNodeFormStore
        .getState()
        .loadNodeData('node-1', [
          { type: 'newField', value: 'newValue', valueType: 'immediate' },
        ]);

      expect(
        useNodeFormStore.getState().getFieldEntry('node-1', 'oldField')
      ).toBeUndefined();
      expect(
        useNodeFormStore.getState().getFieldEntry('node-1', 'newField')?.value
      ).toBe('newValue');
    });
  });

  describe('addCustomField', () => {
    it('adds a new custom field with type hint', () => {
      useNodeFormStore
        .getState()
        .addCustomField('node-1', 'customField', 'json');

      const entry = useNodeFormStore
        .getState()
        .getFieldEntry('node-1', 'customField');
      expect(entry?.type).toBe('customField');
      expect(entry?.value).toBe('');
      expect(entry?.valueType).toBe('immediate');
      expect(entry?.typeHint).toBe('json');
    });

    it('does not overwrite existing field', () => {
      useNodeFormStore
        .getState()
        .setFieldValue('node-1', 'field', 'existingValue');
      useNodeFormStore.getState().addCustomField('node-1', 'field', 'int');

      const entry = useNodeFormStore
        .getState()
        .getFieldEntry('node-1', 'field');
      expect(entry?.value).toBe('existingValue');
      expect(entry?.typeHint).toBeUndefined();
    });
  });

  describe('renameField', () => {
    it('renames a field', () => {
      useNodeFormStore.getState().setFieldValue('node-1', 'oldName', 'value');
      useNodeFormStore
        .getState()
        .setFieldTypeHint('node-1', 'oldName', 'string');

      useNodeFormStore.getState().renameField('node-1', 'oldName', 'newName');

      expect(
        useNodeFormStore.getState().getFieldEntry('node-1', 'oldName')
      ).toBeUndefined();
      const newEntry = useNodeFormStore
        .getState()
        .getFieldEntry('node-1', 'newName');
      expect(newEntry?.type).toBe('newName');
      expect(newEntry?.value).toBe('value');
      expect(newEntry?.typeHint).toBe('string');
    });

    it('does not rename if source does not exist', () => {
      useNodeFormStore
        .getState()
        .renameField('node-1', 'nonExistent', 'newName');

      expect(
        useNodeFormStore.getState().getFieldEntry('node-1', 'newName')
      ).toBeUndefined();
    });

    it('does not rename if target already exists', () => {
      useNodeFormStore
        .getState()
        .setFieldValue('node-1', 'source', 'sourceValue');
      useNodeFormStore
        .getState()
        .setFieldValue('node-1', 'target', 'targetValue');

      useNodeFormStore.getState().renameField('node-1', 'source', 'target');

      expect(
        useNodeFormStore.getState().getFieldEntry('node-1', 'source')?.value
      ).toBe('sourceValue');
      expect(
        useNodeFormStore.getState().getFieldEntry('node-1', 'target')?.value
      ).toBe('targetValue');
    });
  });
});
