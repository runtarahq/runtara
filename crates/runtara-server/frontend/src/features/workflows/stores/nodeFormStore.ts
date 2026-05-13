import { create } from 'zustand';
import { devtools } from 'zustand/middleware';
import { immer } from 'zustand/middleware/immer';

/**
 * Value type for mapping entries
 * - 'immediate': Literal value (string, number, boolean, object, array)
 * - 'reference': Reference to data path (e.g., "steps['step1'].outputs.result")
 * - 'composite': Structured object/array with nested MappingValues
 */
export type InputMappingValueType =
  | 'immediate'
  | 'reference'
  | 'composite'
  | 'template';

/**
 * Supported type hints for immediate values within composites
 */
export type CompositeImmediateTypeHint =
  | 'string'
  | 'integer'
  | 'number'
  | 'boolean'
  | 'file'
  | 'json'
  | 'auto';

/**
 * A single value within a composite structure.
 * Each nested value can be immediate, reference, or another composite.
 */
export type CompositeValue =
  | {
      valueType: 'immediate';
      value: string | number | boolean | null;
      typeHint?: CompositeImmediateTypeHint;
    }
  | { valueType: 'reference'; value: string }
  | {
      valueType: 'composite';
      value: CompositeObjectValue | CompositeArrayValue;
    };

/**
 * Object composite: Record of field names to CompositeValues
 */
export type CompositeObjectValue = Record<string, CompositeValue>;

/**
 * Array composite: Array of CompositeValues
 */
export type CompositeArrayValue = CompositeValue[];

/**
 * Helper to check if a value is a CompositeValue
 */
export function isCompositeValue(value: unknown): value is CompositeValue {
  return (
    typeof value === 'object' &&
    value !== null &&
    'valueType' in value &&
    (value.valueType === 'immediate' ||
      value.valueType === 'reference' ||
      value.valueType === 'composite')
  );
}

export type InputMappingEntry = {
  type: string; // field name
  value:
    | string
    | number
    | boolean
    | null
    | object
    | CompositeObjectValue
    | CompositeArrayValue;
  valueType: InputMappingValueType;
  typeHint?: string; // Type hint for value conversion (e.g., 'integer', 'number', 'boolean', 'string', 'json', 'auto')
};

type NodeFormState = {
  // Map of nodeId -> inputMapping entries (keyed by field name)
  nodeInputMappings: Record<string, Record<string, InputMappingEntry>>;

  // Actions
  setFieldValue: (
    nodeId: string,
    fieldName: string,
    value: InputMappingEntry['value']
  ) => void;
  setFieldValueType: (
    nodeId: string,
    fieldName: string,
    valueType: InputMappingValueType
  ) => void;
  setFieldTypeHint: (
    nodeId: string,
    fieldName: string,
    typeHint: string
  ) => void;
  initializeField: (
    nodeId: string,
    fieldName: string,
    entry: InputMappingEntry
  ) => void;
  removeField: (nodeId: string, fieldName: string) => void;
  getFieldEntry: (
    nodeId: string,
    fieldName: string
  ) => InputMappingEntry | undefined;
  getNodeInputMapping: (nodeId: string) => InputMappingEntry[];
  clearNode: (nodeId: string) => void;
  loadNodeData: (nodeId: string, entries: InputMappingEntry[]) => void;
  // Custom field actions
  addCustomField: (nodeId: string, fieldName: string, typeHint: string) => void;
  renameField: (nodeId: string, oldName: string, newName: string) => void;
};

export const useNodeFormStore = create<NodeFormState>()(
  devtools(
    immer((set, get) => ({
      nodeInputMappings: {},

      setFieldValue: (nodeId, fieldName, value) => {
        set((state) => {
          if (!state.nodeInputMappings[nodeId]) {
            state.nodeInputMappings[nodeId] = {};
          }
          if (!state.nodeInputMappings[nodeId][fieldName]) {
            state.nodeInputMappings[nodeId][fieldName] = {
              type: fieldName,
              value: '',
              valueType: 'immediate',
            };
          }
          state.nodeInputMappings[nodeId][fieldName].value = value;
        });
      },

      setFieldValueType: (nodeId, fieldName, valueType) => {
        set((state) => {
          if (!state.nodeInputMappings[nodeId]) {
            state.nodeInputMappings[nodeId] = {};
          }
          if (!state.nodeInputMappings[nodeId][fieldName]) {
            state.nodeInputMappings[nodeId][fieldName] = {
              type: fieldName,
              value: '',
              valueType: 'immediate',
            };
          }
          state.nodeInputMappings[nodeId][fieldName].valueType = valueType;
        });
      },

      setFieldTypeHint: (nodeId, fieldName, typeHint) => {
        set((state) => {
          if (!state.nodeInputMappings[nodeId]) {
            state.nodeInputMappings[nodeId] = {};
          }
          if (!state.nodeInputMappings[nodeId][fieldName]) {
            state.nodeInputMappings[nodeId][fieldName] = {
              type: fieldName,
              value: '',
              valueType: 'immediate',
            };
          }
          state.nodeInputMappings[nodeId][fieldName].typeHint = typeHint;
        });
      },

      initializeField: (nodeId, fieldName, entry) => {
        set((state) => {
          if (!state.nodeInputMappings[nodeId]) {
            state.nodeInputMappings[nodeId] = {};
          }
          // Don't overwrite if already exists
          if (!state.nodeInputMappings[nodeId][fieldName]) {
            state.nodeInputMappings[nodeId][fieldName] = entry;
          }
        });
      },

      removeField: (nodeId, fieldName) => {
        set((state) => {
          if (state.nodeInputMappings[nodeId]) {
            delete state.nodeInputMappings[nodeId][fieldName];
          }
        });
      },

      getFieldEntry: (nodeId, fieldName) => {
        return get().nodeInputMappings[nodeId]?.[fieldName];
      },

      getNodeInputMapping: (nodeId) => {
        const nodeData = get().nodeInputMappings[nodeId] || {};
        return Object.values(nodeData);
      },

      clearNode: (nodeId) => {
        set((state) => {
          delete state.nodeInputMappings[nodeId];
        });
      },

      loadNodeData: (nodeId, entries) => {
        set((state) => {
          state.nodeInputMappings[nodeId] = {};
          entries.forEach((entry) => {
            state.nodeInputMappings[nodeId][entry.type] = entry;
          });
        });
      },

      addCustomField: (nodeId, fieldName, typeHint) => {
        set((state) => {
          if (!state.nodeInputMappings[nodeId]) {
            state.nodeInputMappings[nodeId] = {};
          }
          // Don't overwrite if field already exists
          if (!state.nodeInputMappings[nodeId][fieldName]) {
            state.nodeInputMappings[nodeId][fieldName] = {
              type: fieldName,
              value: '',
              valueType: 'immediate',
              typeHint,
            };
          }
        });
      },

      renameField: (nodeId, oldName, newName) => {
        set((state) => {
          const nodeData = state.nodeInputMappings[nodeId];
          if (!nodeData || !nodeData[oldName] || nodeData[newName]) {
            return; // Don't rename if source doesn't exist or target exists
          }

          nodeData[newName] = { ...nodeData[oldName], type: newName };
          delete nodeData[oldName];
        });
      },
    })),
    { name: 'node-form-store' }
  )
);
