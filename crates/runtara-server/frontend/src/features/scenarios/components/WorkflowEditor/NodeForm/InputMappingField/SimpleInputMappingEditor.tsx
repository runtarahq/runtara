/**
 * Simple, stateless input mapping editor using Zustand for state.
 *
 * All state is stored in useNodeFormStore, keyed by nodeId and fieldName.
 * This component just renders the UI and dispatches actions.
 */

import React, {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import { shallow } from 'zustand/shallow';
import { Icons } from '@/shared/components/icons';
import { Button } from '@/shared/components/ui/button';
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/shared/components/ui/table';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/shared/components/ui/dropdown-menu';
import type { CapabilityField } from '@/generated/RuntaraRuntimeApi';
import {
  useNodeFormStore,
  InputMappingEntry,
} from '@/features/scenarios/stores/nodeFormStore';
import { useWorkflowStore } from '@/features/scenarios/stores/workflowStore';
import {
  getFieldLabel,
  getFieldHelpText,
  getFieldPlaceholder,
  getFieldInitialValue,
  isEnumField,
  getInputComponentType,
} from '@/features/scenarios/types/agent-metadata';
import { MappingValueInput, ValueMode } from './MappingValueInput';
import type {
  InputMappingValueType,
  CompositeObjectValue,
  CompositeArrayValue,
} from '@/features/scenarios/stores/nodeFormStore';
import { FileInputWithReferences } from './FileInputWithReferences';
import { ArrayMappingEditor } from './ArrayMappingEditor';
import { ObjectMappingEditor } from './ObjectMappingEditor';
import { CustomFieldRow } from './CustomFieldRow';
import { AddCustomFieldDialog } from './AddCustomFieldDialog';
import { useTabContext } from '../NodeFormItem';

/** Check if a field type is an array type */
function isArrayType(type: string | undefined): boolean {
  if (!type) return false;
  const lowerType = type.toLowerCase();
  return (
    lowerType === 'array' ||
    lowerType.startsWith('array<') ||
    lowerType.startsWith('[') ||
    lowerType.includes('[]')
  );
}

/** Check if a field type is an object type (but not array) */
function isObjectType(type: string | undefined): boolean {
  if (!type) return false;
  const lowerType = type.toLowerCase();
  // Exclude array types that might contain 'object'
  if (isArrayType(type)) return false;
  return lowerType === 'object' || lowerType.startsWith('{');
}

/** Check if a field type is untyped (any, unknown, or missing type) */
function isUntypedField(type: string | undefined): boolean {
  if (!type) return true;
  const lowerType = type.toLowerCase();
  return lowerType === 'any' || lowerType === 'unknown' || lowerType === '';
}

/**
 * Collect legacy dot-notation entries for an object field
 * e.g., for field "context", collects entries like "context.index", "context.item"
 * Returns array of { path: "index", value: ... }, { path: "item", value: ... }
 */
function collectLegacyObjectFields(
  nodeData: Record<string, InputMappingEntry>,
  fieldName: string
): Array<{ path: string; value: any }> {
  const prefix = `${fieldName}.`;
  const result: Array<{ path: string; value: any }> = [];

  Object.entries(nodeData).forEach(([key, entry]) => {
    if (key.startsWith(prefix)) {
      const subPath = key.slice(prefix.length);
      result.push({ path: subPath, value: entry.value });
    }
  });

  return result;
}

/**
 * Maps field type to TypeHint value for proper value conversion
 * This ensures numeric values are converted from strings to actual numbers
 */
function getTypeHintFromFieldType(fieldType: string | undefined): string {
  if (!fieldType) return 'auto';
  const lowerType = fieldType.toLowerCase();

  // String types
  if (lowerType === 'string' || lowerType === 'text' || lowerType === 'str')
    return 'text';

  // Boolean types
  if (lowerType === 'boolean' || lowerType === 'bool') return 'boolean';

  // Integer types - handle various naming conventions from different APIs
  // Returns 'integer' to match ValueType.Integer for proper type conversion
  // Includes 'auto' since Auto column type is auto-incrementing integer
  if (
    lowerType === 'integer' ||
    lowerType === 'int' ||
    lowerType === 'int32' ||
    lowerType === 'int64' ||
    lowerType === 'i32' ||
    lowerType === 'i64' ||
    lowerType === 'long' ||
    lowerType === 'short' ||
    lowerType === 'auto'
  )
    return 'integer';

  // Float/double types
  // Returns 'number' to match ValueType.Number for proper type conversion
  if (
    lowerType === 'number' ||
    lowerType === 'float' ||
    lowerType === 'double' ||
    lowerType === 'f32' ||
    lowerType === 'f64' ||
    lowerType === 'decimal'
  )
    return 'number';

  // Array types
  if (
    lowerType === 'array' ||
    lowerType.startsWith('[') ||
    lowerType.includes('array<') ||
    lowerType.includes('list<') ||
    lowerType.includes('vec<')
  )
    return 'json';

  // Object types
  if (lowerType === 'object' || lowerType.startsWith('{')) return 'json';

  return 'auto';
}

interface SimpleInputMappingEditorProps {
  nodeId: string;
  fields: CapabilityField[];
  /** Initial data to load (from node.data.inputMapping) */
  initialData?: InputMappingEntry[];
  /** Called when data changes - parent can use this to sync back to workflow */
  onDataChange?: (entries: InputMappingEntry[]) => void;
  /** Hide reference mode toggle (for testing/immediate-only contexts) */
  hideReferenceToggle?: boolean;
  /** Allow adding custom fields not defined in the schema (default: false) */
  allowCustomFields?: boolean;
}

// Stable empty object to avoid creating new references on each render
const EMPTY_NODE_DATA: Record<string, InputMappingEntry> = {};

/**
 * Single field row - completely stateless, reads/writes to Zustand store
 */
function FieldRow({
  nodeId,
  field,
  isOptional,
  onRemove,
  onFieldChange,
  hideReferenceToggle = false,
  onEditArray,
  onEditObject,
  onFieldFocus,
  legacyFieldCount = 0,
}: {
  nodeId: string;
  field: CapabilityField;
  isOptional: boolean;
  onRemove?: () => void;
  onFieldChange?: () => void;
  hideReferenceToggle?: boolean;
  onEditArray?: (field: CapabilityField) => void;
  onEditObject?: (field: CapabilityField) => void;
  /** Called when a non-array/non-object field receives focus - used to close editors */
  onFieldFocus?: () => void;
  /** Number of legacy dot-notation fields for this object field */
  legacyFieldCount?: number;
}) {
  const entry = useNodeFormStore((s) => s.getFieldEntry(nodeId, field.name));
  const setFieldValue = useNodeFormStore((s) => s.setFieldValue);
  const setFieldValueType = useNodeFormStore((s) => s.setFieldValueType);

  const label = getFieldLabel(field);
  const helpText = getFieldHelpText(field);
  const placeholder = getFieldPlaceholder(field);
  const componentType = getInputComponentType(field);
  const isEnum = isEnumField(field);
  const enumOptions = isEnum && field.enum ? field.enum.map(String) : undefined;
  const isArray = isArrayType(field.type);
  const isObject = isObjectType(field.type);
  const isUntyped = isUntypedField(field.type);
  // Treat untyped fields as object-like to allow composite mode
  const showObjectEditor = isObject || isUntyped;

  const value = entry?.value ?? '';
  const valueType = (entry?.valueType ?? 'immediate') as InputMappingValueType;

  const handleValueChange = (newValue: string) => {
    setFieldValue(nodeId, field.name, newValue);
    onFieldChange?.();
  };

  const handleValueTypeChange = (newType: ValueMode) => {
    setFieldValueType(nodeId, field.name, newType);
    onFieldChange?.();
  };

  // Get display value for arrays
  const getArrayDisplayValue = () => {
    if (!value) return 'Click to configure...';
    if (valueType === 'reference') return `Reference: ${value}`;
    // Handle composite mode - value is the composite structure directly
    if (valueType === 'composite') {
      if (Array.isArray(value)) {
        return `Composite: ${value.length} item${value.length !== 1 ? 's' : ''}`;
      }
      return 'Composite Array';
    }
    try {
      const parsed = JSON.parse(String(value));
      if (Array.isArray(parsed)) {
        return `${parsed.length} item${parsed.length !== 1 ? 's' : ''}`;
      }
    } catch {
      // Invalid JSON
    }
    return 'Click to configure...';
  };

  // Get display value for objects
  const getObjectDisplayValue = () => {
    // Check for legacy dot-notation fields first
    if (legacyFieldCount > 0) {
      return `${legacyFieldCount} field${legacyFieldCount !== 1 ? 's' : ''}`;
    }
    if (!value) return 'Click to configure...';
    if (valueType === 'reference') return `Reference: ${value}`;
    // Handle composite mode - value is the composite structure directly
    if (valueType === 'composite') {
      if (
        typeof value === 'object' &&
        value !== null &&
        !Array.isArray(value)
      ) {
        const fieldCount = Object.keys(value).length;
        return `Composite: ${fieldCount} field${fieldCount !== 1 ? 's' : ''}`;
      }
      return 'Composite Object';
    }
    try {
      const parsed = JSON.parse(String(value));
      if (
        typeof parsed === 'object' &&
        parsed !== null &&
        !Array.isArray(parsed)
      ) {
        const fieldCount = Object.keys(parsed).length;
        return `${fieldCount} field${fieldCount !== 1 ? 's' : ''}`;
      }
    } catch {
      // Invalid JSON
    }
    return 'Click to configure...';
  };

  return (
    <TableRow className="hover:bg-muted/30">
      {/* Type column */}
      <TableCell className="align-top pt-3">
        <span className="text-[11px] font-mono px-1.5 py-0.5 rounded text-muted-foreground bg-muted/40 truncate block">
          {field.type || 'any'}
        </span>
      </TableCell>

      {/* Name column */}
      <TableCell className="align-top pt-3 overflow-hidden max-w-0">
        <div className="flex items-center gap-1 min-w-0">
          <span
            className="text-sm text-slate-900/90 dark:text-slate-100 truncate"
            title={label}
          >
            {label}
          </span>
          {field.required && (
            <span className="text-destructive text-xs shrink-0">*</span>
          )}
          {helpText && (
            <Icons.info
              className="h-3 w-3 text-muted-foreground cursor-help shrink-0"
              title={helpText}
            />
          )}
        </div>
      </TableCell>

      {/* Value column */}
      <TableCell>
        {isArray ? (
          // Array field - show button to open array editor
          <button
            type="button"
            onClick={() => onEditArray?.(field)}
            className="w-full flex items-center justify-between gap-2 px-3 py-2 text-sm border rounded-md bg-muted/30 hover:bg-muted/50 transition-colors text-left"
          >
            <span className="text-muted-foreground truncate">
              {getArrayDisplayValue()}
            </span>
            <Icons.chevronRight className="h-4 w-4 text-muted-foreground shrink-0" />
          </button>
        ) : showObjectEditor ? (
          // Object/untyped field - show button to open object editor (with composite mode)
          <button
            type="button"
            onClick={() => onEditObject?.(field)}
            className="w-full flex items-center justify-between gap-2 px-3 py-2 text-sm border rounded-md bg-muted/30 hover:bg-muted/50 transition-colors text-left"
          >
            <span className="text-muted-foreground truncate">
              {getObjectDisplayValue()}
            </span>
            <Icons.chevronRight className="h-4 w-4 text-muted-foreground shrink-0" />
          </button>
        ) : componentType === 'file' ? (
          <div onFocus={onFieldFocus}>
            <FileInputWithReferences
              value={typeof value === 'string' ? value : ''}
              onChange={handleValueChange}
              placeholder={placeholder || 'Upload a file'}
            />
          </div>
        ) : (
          <div onFocus={onFieldFocus}>
            <MappingValueInput
              value={
                typeof value === 'object'
                  ? JSON.stringify(value, null, 2)
                  : String(value)
              }
              onChange={handleValueChange}
              valueType={valueType as ValueMode}
              onValueTypeChange={handleValueTypeChange}
              fieldType={componentType}
              fieldName={field.name}
              placeholder={placeholder}
              enumOptions={enumOptions}
              hideReferenceToggle={hideReferenceToggle}
            />
          </div>
        )}
      </TableCell>

      {/* Actions column */}
      <TableCell className="align-top pt-2">
        {isOptional && onRemove && (
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="h-7 w-7 text-muted-foreground hover:text-destructive"
            onClick={onRemove}
            title="Remove field"
          >
            <Icons.remove className="h-3.5 w-3.5" />
          </Button>
        )}
      </TableCell>
    </TableRow>
  );
}

/**
 * Main editor component
 */
export function SimpleInputMappingEditor({
  nodeId,
  fields,
  initialData,
  onDataChange,
  hideReferenceToggle = false,
  allowCustomFields = false,
}: SimpleInputMappingEditorProps) {
  const loadNodeData = useNodeFormStore((s) => s.loadNodeData);
  const initializeField = useNodeFormStore((s) => s.initializeField);
  const removeField = useNodeFormStore((s) => s.removeField);
  const getNodeInputMapping = useNodeFormStore((s) => s.getNodeInputMapping);
  const clearNode = useNodeFormStore((s) => s.clearNode);
  // Use shallow equality to prevent re-renders when the object reference changes but content is the same
  // Use stable EMPTY_NODE_DATA constant to avoid creating new object references
  const nodeData = useNodeFormStore(
    (s) => s.nodeInputMappings[nodeId] ?? EMPTY_NODE_DATA,
    shallow
  );
  const setFieldValue = useNodeFormStore((s) => s.setFieldValue);
  const setFieldValueType = useNodeFormStore((s) => s.setFieldValueType);
  const addCustomField = useNodeFormStore((s) => s.addCustomField);
  const renameField = useNodeFormStore((s) => s.renameField);

  // Array editing state - track which array field is expanded inline
  const [editingArrayFieldName, setEditingArrayFieldName] = useState<
    string | null
  >(null);

  // Object editing state - track which object field is expanded inline
  const [editingObjectFieldName, setEditingObjectFieldName] = useState<
    string | null
  >(null);

  // Custom field dialog state
  const [isAddCustomFieldOpen, setIsAddCustomFieldOpen] = useState(false);

  // Get tab context to close editors when switching tabs
  const { activeTab } = useTabContext();

  // Close array/object editors when switching to testing tab
  // Only depends on activeTab - we only care about tab changes, not about the editor state
  useEffect(() => {
    if (activeTab !== 'main') {
      setEditingArrayFieldName(null);
      setEditingObjectFieldName(null);
    }
  }, [activeTab]);

  // Use refs to store latest callback references for unmount effect
  const onDataChangeRef = useRef(onDataChange);
  const getNodeInputMappingRef = useRef(getNodeInputMapping);

  // Keep refs updated - use assignment instead of useEffect to avoid re-render cycles
  onDataChangeRef.current = onDataChange;
  getNodeInputMappingRef.current = getNodeInputMapping;

  // Sync to parent on unmount to ensure values are saved when sidebar closes
  // Also clean up temporary node data on unmount (for create mode)
  useEffect(() => {
    const currentNodeId = nodeId;
    return () => {
      // Sync final state to parent before unmounting
      if (onDataChangeRef.current) {
        const entries = getNodeInputMappingRef.current(currentNodeId);
        onDataChangeRef.current(entries);
      }
      // Clean up temporary node data
      if (currentNodeId.startsWith('__temp_')) {
        clearNode(currentNodeId);
      }
    };
  }, [nodeId, clearNode]);

  // Sync Zustand store changes to react-hook-form via onDataChange callback
  const syncToParent = useCallback(() => {
    if (onDataChange) {
      const entries = getNodeInputMapping(nodeId);
      onDataChange(entries);
    }
  }, [onDataChange, getNodeInputMapping, nodeId]);

  // Separate required and optional fields
  const requiredFields = useMemo(
    () => fields.filter((f) => f.required),
    [fields]
  );
  const optionalFields = useMemo(
    () => fields.filter((f) => !f.required),
    [fields]
  );

  // Track which optional fields are visible (have entries in store)
  const visibleOptionalFieldNames = useMemo(() => {
    return new Set(
      optionalFields
        .filter((f) => nodeData[f.name] !== undefined)
        .map((f) => f.name)
    );
  }, [optionalFields, nodeData]);

  // Available optional fields to add
  const availableOptionalFields = useMemo(
    () => optionalFields.filter((f) => !visibleOptionalFieldNames.has(f.name)),
    [optionalFields, visibleOptionalFieldNames]
  );

  // Fields to render
  const visibleFields = useMemo(() => {
    const visible = optionalFields.filter((f) =>
      visibleOptionalFieldNames.has(f.name)
    );
    return [...requiredFields, ...visible];
  }, [requiredFields, optionalFields, visibleOptionalFieldNames]);

  // Set of all field names from metadata (for identifying custom fields)
  const metadataFieldNames = useMemo(() => {
    return new Set(fields.map((f) => f.name));
  }, [fields]);

  // Set of object field names (for filtering out legacy dot-notation entries)
  const objectFieldNames = useMemo(() => {
    return new Set(
      fields.filter((f) => isObjectType(f.type)).map((f) => f.name)
    );
  }, [fields]);

  // Check if a field name is a legacy dot-notation entry for an object field
  // e.g., "context.index" is a legacy entry for object field "context"
  const isLegacyObjectEntry = useCallback(
    (name: string): boolean => {
      const dotIndex = name.indexOf('.');
      if (dotIndex === -1) return false;
      const baseFieldName = name.slice(0, dotIndex);
      return objectFieldNames.has(baseFieldName);
    },
    [objectFieldNames]
  );

  // Custom fields are entries in nodeData that don't match any field in the schema
  // Also exclude legacy dot-notation entries (e.g., "context.index" for object field "context")
  const customFieldNames = useMemo(() => {
    if (!allowCustomFields) return [];
    return Object.keys(nodeData).filter(
      (name) => !metadataFieldNames.has(name) && !isLegacyObjectEntry(name)
    );
  }, [nodeData, metadataFieldNames, allowCustomFields, isLegacyObjectEntry]);

  // All existing field names (for validation when adding/renaming)
  const allExistingFieldNames = useMemo(() => {
    const names = new Set<string>();
    // Add metadata field names
    fields.forEach((f) => names.add(f.name));
    // Add current custom field names
    customFieldNames.forEach((name) => names.add(name));
    return names;
  }, [fields, customFieldNames]);

  // Track if we've done initial load to avoid triggering onDataChange during setup
  const isInitializedRef = useRef(false);
  const previousNodeDataRef = useRef<string>('');

  // Create a map for quick lookup of initial data values
  const initialDataMap = useMemo(() => {
    const map = new Map<string, InputMappingEntry>();
    if (initialData) {
      initialData.forEach((entry) => {
        map.set(entry.type, entry);
      });
    }
    return map;
  }, [initialData]);

  // Create a map of field name -> field definition for quick lookup
  const fieldDefMap = useMemo(() => {
    const map = new Map<string, CapabilityField>();
    fields.forEach((field) => {
      map.set(field.name, field);
    });
    return map;
  }, [fields]);

  // Load initial data when it changes
  // Ensure typeHint is set from field definitions if not present in initialData
  // IMPORTANT: Always load data (even if empty) to clear stale data from previous sessions
  useEffect(() => {
    if (initialData && initialData.length > 0) {
      // Enrich entries with typeHint from field definitions if missing
      const enrichedData = initialData.map((entry) => {
        const fieldDef = fieldDefMap.get(entry.type);
        const computedTypeHint = fieldDef
          ? getTypeHintFromFieldType(fieldDef.type)
          : 'auto';
        return {
          ...entry,
          // Only use computedTypeHint if entry.typeHint is undefined or 'auto'
          typeHint:
            entry.typeHint && entry.typeHint !== 'auto'
              ? entry.typeHint
              : computedTypeHint,
        };
      });
      loadNodeData(nodeId, enrichedData);
    } else {
      // Clear any stale data from previous sessions when initialData is empty
      loadNodeData(nodeId, []);
    }
    // Mark as initialized after a tick to let initial data settle
    const timer = setTimeout(() => {
      isInitializedRef.current = true;
      // Store initial state for comparison
      previousNodeDataRef.current = JSON.stringify(nodeData);
    }, 50);
    return () => clearTimeout(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps -- nodeData is intentionally excluded to prevent infinite loops
  }, [nodeId, initialData, loadNodeData, fieldDefMap]);

  // Initialize required fields if not already present
  // Use values from initialData if available, otherwise leave empty (not defaults)
  useEffect(() => {
    requiredFields.forEach((field) => {
      // Check if we have an existing value from initialData
      const existingEntry = initialDataMap.get(field.name);
      if (existingEntry) {
        // Use existing value from initialData, ensure typeHint is set
        initializeField(nodeId, field.name, {
          ...existingEntry,
          typeHint:
            existingEntry.typeHint || getTypeHintFromFieldType(field.type),
        });
      } else {
        // No existing value - use field default if available (especially for enum fields),
        // otherwise initialize empty
        const defaultValue =
          field.default !== undefined && field.default !== null
            ? field.default
            : field.enum && field.enum.length > 0
              ? field.enum[0]
              : '';
        initializeField(nodeId, field.name, {
          type: field.name,
          value: defaultValue,
          valueType: 'immediate',
          typeHint: getTypeHintFromFieldType(field.type),
        });
      }
    });
  }, [nodeId, requiredFields, initializeField, initialDataMap]);

  // Called when any field value changes - mark node as staged for highlighting and sync to parent
  const handleFieldChange = useCallback(() => {
    // Directly mark this node as having staged changes for visual highlighting
    const currentStagedIds = useWorkflowStore.getState().stagedNodeIds;
    if (!currentStagedIds.has(nodeId)) {
      const newStagedIds = new Set(currentStagedIds);
      newStagedIds.add(nodeId);
      useWorkflowStore.getState().setStagedNodeIds(newStagedIds);
    }
    // Sync changes to react-hook-form so they are included in staged changes
    syncToParent();
  }, [nodeId, syncToParent]);

  const handleAddOptionalField = (fieldName: string) => {
    const fieldDef = optionalFields.find((f) => f.name === fieldName);
    if (fieldDef) {
      const initialValue = getFieldInitialValue(fieldDef, undefined);
      initializeField(nodeId, fieldName, {
        type: fieldName,
        value: initialValue ?? '',
        valueType: 'immediate',
        typeHint: getTypeHintFromFieldType(fieldDef.type),
      });
      // Sync to parent after adding field
      // Use setTimeout to ensure store is updated before syncing
      setTimeout(() => syncToParent(), 0);
    }
  };

  const handleRemoveOptionalField = (fieldName: string) => {
    removeField(nodeId, fieldName);
    // Sync to parent after removing field
    // Use setTimeout to ensure store is updated before syncing
    setTimeout(() => syncToParent(), 0);
  };

  // Custom field handlers
  const handleAddCustomField = useCallback(
    (fieldName: string, typeHint: string) => {
      addCustomField(nodeId, fieldName, typeHint);
      // Sync to parent after adding field
      setTimeout(() => syncToParent(), 0);
    },
    [nodeId, addCustomField, syncToParent]
  );

  const handleRemoveCustomField = useCallback(
    (fieldName: string) => {
      removeField(nodeId, fieldName);
      setTimeout(() => syncToParent(), 0);
    },
    [nodeId, removeField, syncToParent]
  );

  const handleRenameCustomField = useCallback(
    (oldName: string, newName: string) => {
      renameField(nodeId, oldName, newName);
      handleFieldChange();
    },
    [nodeId, renameField, handleFieldChange]
  );

  // Array editing handlers
  const handleEditArray = useCallback((field: CapabilityField) => {
    // Toggle: if already editing this field, close it; otherwise open it
    setEditingArrayFieldName((prev) =>
      prev === field.name ? null : field.name
    );
  }, []);

  const handleArrayClose = useCallback(() => {
    setEditingArrayFieldName(null);
    // Sync changes when closing
    handleFieldChange();
  }, [handleFieldChange]);

  const handleArrayValueChange = useCallback(
    (
      fieldName: string,
      value: string | CompositeObjectValue | CompositeArrayValue
    ) => {
      // If the value is an object/array, ensure valueType is set to 'composite'
      const isObjectValue = typeof value === 'object' && value !== null;
      if (isObjectValue) {
        setFieldValueType(nodeId, fieldName, 'composite');
      }
      setFieldValue(nodeId, fieldName, value);
      // Sync changes immediately - Zustand updates are synchronous
      syncToParent();
    },
    [nodeId, setFieldValue, setFieldValueType, syncToParent]
  );

  const handleArrayValueTypeChange = useCallback(
    (fieldName: string, valueType: InputMappingValueType) => {
      setFieldValueType(nodeId, fieldName, valueType);
      // Sync changes immediately
      syncToParent();
    },
    [nodeId, setFieldValueType, syncToParent]
  );

  // Object editing handlers
  const handleEditObject = useCallback(
    (field: CapabilityField) => {
      // Toggle: if already editing this field, close it; otherwise open it
      setEditingObjectFieldName((prev) =>
        prev === field.name ? null : field.name
      );
      // Close array editor if open
      if (editingArrayFieldName) {
        setEditingArrayFieldName(null);
      }
    },
    [editingArrayFieldName]
  );

  const handleObjectClose = useCallback(() => {
    setEditingObjectFieldName(null);
    // Sync changes when closing
    handleFieldChange();
  }, [handleFieldChange]);

  const handleObjectValueChange = useCallback(
    (
      fieldName: string,
      value: string | CompositeObjectValue | CompositeArrayValue
    ) => {
      const isObjectValue = typeof value === 'object' && value !== null;
      console.log('[SimpleInputMappingEditor] handleObjectValueChange called', {
        nodeId,
        fieldName,
        value,
        valueType: typeof value,
        isObject: isObjectValue,
      });

      // If the value is an object/array, we need to set valueType to 'composite'
      // This ensures the value is properly passed back as an object on re-render
      if (isObjectValue) {
        setFieldValueType(nodeId, fieldName, 'composite');
      }

      setFieldValue(nodeId, fieldName, value);
      // Sync changes immediately - Zustand updates are synchronous
      syncToParent();
    },
    [nodeId, setFieldValue, setFieldValueType, syncToParent]
  );

  const handleObjectValueTypeChange = useCallback(
    (fieldName: string, valueType: InputMappingValueType) => {
      setFieldValueType(nodeId, fieldName, valueType);
      // Sync changes immediately
      syncToParent();
    },
    [nodeId, setFieldValueType, syncToParent]
  );

  // Legacy object fields handler - saves as separate dot-notation entries
  const handleLegacyObjectFieldsChange = useCallback(
    (fieldName: string, fields: Array<{ path: string; value: any }>) => {
      // Get current legacy fields to find which ones to remove
      const existingLegacyFields = collectLegacyObjectFields(
        nodeData,
        fieldName
      );
      const existingPaths = new Set(existingLegacyFields.map((f) => f.path));
      const newPaths = new Set(fields.map((f) => f.path));

      // Remove fields that no longer exist
      existingPaths.forEach((path) => {
        if (!newPaths.has(path)) {
          removeField(nodeId, `${fieldName}.${path}`);
        }
      });

      // Add/update fields
      fields.forEach((field) => {
        if (field.path) {
          const fullKey = `${fieldName}.${field.path}`;
          setFieldValue(nodeId, fullKey, field.value);
        }
      });

      // Sync changes immediately
      syncToParent();
    },
    [nodeId, nodeData, setFieldValue, removeField, syncToParent]
  );

  // Previously closed array/object editor when clicking on other fields,
  // but now we keep it open to allow editing multiple fields simultaneously.
  // The user can explicitly close the editor using the close button.
  const handleFieldFocus = useCallback(() => {
    // No-op: don't close the editor when focusing on other fields
  }, []);

  // Empty state when no fields and no custom fields
  const hasNoFields = fields.length === 0 && customFieldNames.length === 0;

  if (hasNoFields && !allowCustomFields) {
    return (
      <div className="text-sm text-muted-foreground p-6 rounded-xl bg-muted/20 text-center">
        No input fields required for this operation
      </div>
    );
  }

  return (
    <div className="space-y-4">
      <div className="rounded-xl bg-card overflow-hidden">
        <Table className="w-full" style={{ tableLayout: 'fixed' }}>
          <colgroup>
            <col style={{ width: '80px' }} />
            <col style={{ width: '180px' }} />
            <col />
            <col style={{ width: '48px' }} />
          </colgroup>
          <TableHeader>
            <TableRow className="hover:bg-transparent border-b border-border/40">
              <TableHead className="text-xs font-medium text-muted-foreground">
                Type
              </TableHead>
              <TableHead className="text-xs font-medium text-muted-foreground">
                Parameter
              </TableHead>
              <TableHead className="text-xs font-medium text-muted-foreground">
                Value
              </TableHead>
              <TableHead />
            </TableRow>
          </TableHeader>
          <TableBody>
            {visibleFields.length > 0 ? (
              visibleFields.map((field) => {
                const isEditingThisArray = editingArrayFieldName === field.name;
                const isEditingThisObject =
                  editingObjectFieldName === field.name;
                const fieldEntry = nodeData[field.name];
                // Collect legacy dot-notation fields for object types
                const legacyFields = isObjectType(field.type)
                  ? collectLegacyObjectFields(nodeData, field.name)
                  : [];

                return (
                  <React.Fragment key={field.name}>
                    <FieldRow
                      nodeId={nodeId}
                      field={field}
                      isOptional={!field.required}
                      onRemove={
                        !field.required
                          ? () => handleRemoveOptionalField(field.name)
                          : undefined
                      }
                      onFieldChange={handleFieldChange}
                      hideReferenceToggle={hideReferenceToggle}
                      onEditArray={handleEditArray}
                      onEditObject={handleEditObject}
                      onFieldFocus={handleFieldFocus}
                      legacyFieldCount={legacyFields.length}
                    />
                    {/* Inline array editor - appears below the field row */}
                    {isEditingThisArray && isArrayType(field.type) && (
                      <TableRow className="hover:bg-transparent">
                        <TableCell colSpan={4} className="p-0 border-t-0">
                          <div className="border-t border-primary/20 bg-muted/20">
                            <ArrayMappingEditor
                              arrayType={field.type || 'array'}
                              value={
                                fieldEntry?.valueType === 'composite'
                                  ? (fieldEntry.value as
                                      | CompositeObjectValue
                                      | CompositeArrayValue)
                                  : String(fieldEntry?.value ?? '')
                              }
                              valueType={
                                (fieldEntry?.valueType ??
                                  'immediate') as InputMappingValueType
                              }
                              onChange={(value) =>
                                handleArrayValueChange(field.name, value)
                              }
                              onValueTypeChange={(type) =>
                                handleArrayValueTypeChange(field.name, type)
                              }
                              itemSchema={field.items as any}
                              onClose={handleArrayClose}
                            />
                          </div>
                        </TableCell>
                      </TableRow>
                    )}
                    {/* Inline object editor - appears below the field row (also for untyped fields or composite mode) */}
                    {((isEditingThisObject &&
                      (isObjectType(field.type) ||
                        isUntypedField(field.type))) ||
                      (fieldEntry?.valueType === 'composite' &&
                        !isArrayType(field.type))) &&
                      (() => {
                        // Determine the value and valueType for the editor
                        // If the value is already an object/array, treat it as composite even if valueType says otherwise
                        const isValueObject =
                          typeof fieldEntry?.value === 'object' &&
                          fieldEntry?.value !== null;
                        const effectiveValueType: InputMappingValueType =
                          fieldEntry?.valueType === 'composite' ||
                          fieldEntry?.valueType === 'reference'
                            ? fieldEntry.valueType
                            : isValueObject
                              ? 'composite'
                              : 'immediate';

                        // Get the value for the editor
                        // If valueType is composite OR the value is actually an object, pass it directly
                        // Otherwise convert to string for immediate/reference modes
                        const valueForEditor =
                          effectiveValueType === 'composite'
                            ? ((isValueObject ? fieldEntry.value : {}) as
                                | CompositeObjectValue
                                | CompositeArrayValue)
                            : effectiveValueType === 'reference'
                              ? String(fieldEntry?.value ?? '')
                              : String(fieldEntry?.value ?? '');

                        return (
                          <TableRow className="hover:bg-transparent">
                            <TableCell colSpan={4} className="p-0 border-t-0">
                              <div className="border-t border-primary/20 bg-muted/20">
                                <ObjectMappingEditor
                                  value={valueForEditor}
                                  valueType={effectiveValueType}
                                  onChange={(value) =>
                                    handleObjectValueChange(field.name, value)
                                  }
                                  onValueTypeChange={(type) =>
                                    handleObjectValueTypeChange(
                                      field.name,
                                      type
                                    )
                                  }
                                  schema={field.items as any}
                                  onClose={handleObjectClose}
                                  legacyFields={legacyFields}
                                  onLegacyFieldsChange={(fields) =>
                                    handleLegacyObjectFieldsChange(
                                      field.name,
                                      fields
                                    )
                                  }
                                />
                              </div>
                            </TableCell>
                          </TableRow>
                        );
                      })()}
                  </React.Fragment>
                );
              })
            ) : visibleFields.length === 0 && customFieldNames.length === 0 ? (
              <TableRow>
                <TableCell
                  colSpan={4}
                  className="text-center text-muted-foreground py-8"
                >
                  {allowCustomFields
                    ? 'No parameters defined. Add custom parameters below.'
                    : 'No required parameters. Add optional parameters below.'}
                </TableCell>
              </TableRow>
            ) : null}

            {/* Custom fields section */}
            {allowCustomFields && customFieldNames.length > 0 && (
              <>
                {customFieldNames.map((fieldName) => (
                  <CustomFieldRow
                    key={fieldName}
                    nodeId={nodeId}
                    fieldName={fieldName}
                    fieldType={nodeData[fieldName]?.typeHint || 'string'}
                    onRemove={() => handleRemoveCustomField(fieldName)}
                    onFieldChange={handleFieldChange}
                    onRename={handleRenameCustomField}
                    existingFieldNames={allExistingFieldNames}
                    hideReferenceToggle={hideReferenceToggle}
                  />
                ))}
              </>
            )}
          </TableBody>
        </Table>
      </div>

      {availableOptionalFields.length > 0 && (
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="w-full text-muted-foreground hover:text-foreground"
            >
              <Icons.add className="h-4 w-4 mr-2" />
              Add optional parameter ({availableOptionalFields.length}{' '}
              available)
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent
            align="start"
            className="w-80 max-h-64 overflow-y-auto"
          >
            {availableOptionalFields.map((field) => (
              <DropdownMenuItem
                key={field.name}
                onClick={() => handleAddOptionalField(field.name)}
                className="flex items-center justify-between"
              >
                <div className="flex flex-col">
                  <span className="text-sm text-slate-900/90 dark:text-slate-100">
                    {getFieldLabel(field)}
                  </span>
                  {field.description && (
                    <span className="text-xs text-muted-foreground truncate max-w-60">
                      {field.description}
                    </span>
                  )}
                </div>
                <span className="text-[10px] font-mono px-1.5 py-0.5 rounded text-muted-foreground bg-muted/40 ml-2 shrink-0">
                  {field.type || 'any'}
                </span>
              </DropdownMenuItem>
            ))}
          </DropdownMenuContent>
        </DropdownMenu>
      )}

      {/* Add custom parameter button */}
      {allowCustomFields && (
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="w-full text-muted-foreground hover:text-foreground border-dashed"
          onClick={() => setIsAddCustomFieldOpen(true)}
        >
          <Icons.add className="h-4 w-4 mr-2" />
          Add custom parameter
        </Button>
      )}

      {/* Add custom field dialog */}
      <AddCustomFieldDialog
        open={isAddCustomFieldOpen}
        onOpenChange={setIsAddCustomFieldOpen}
        onAdd={handleAddCustomField}
        existingFieldNames={allExistingFieldNames}
      />
    </div>
  );
}
