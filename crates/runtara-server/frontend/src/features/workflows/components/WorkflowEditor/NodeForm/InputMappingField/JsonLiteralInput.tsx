import { MappingValueInput, type ValueMode } from './MappingValueInput';

/**
 * The JSON box an inline object or array editor shows in Value mode for a
 * literal structure (see json-literal.ts).
 */
export function JsonLiteralInput({
  text,
  onEdit,
  onValueTypeChange,
}: {
  text: string;
  onEdit: (text: string) => void;
  onValueTypeChange: (type: ValueMode) => void;
}) {
  return (
    <div className="space-y-2">
      <p className="text-sm text-muted-foreground">
        A JSON value, sent as written.
      </p>
      <MappingValueInput
        value={text}
        onChange={(v) => onEdit(v ?? '')}
        valueType="immediate"
        onValueTypeChange={onValueTypeChange}
        fieldType="json"
        hideReferenceToggle
        placeholder="Enter a JSON value..."
      />
      {text === '' && (
        <p className="text-xs text-muted-foreground">
          Empty: saving the step leaves this input out.
        </p>
      )}
    </div>
  );
}
