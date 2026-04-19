export type ValueType =
  | 'reference'
  | 'immediate'
  | 'composite-object'
  | 'composite-array'
  | 'template';

/** Base value type without composite distinction (for store compatibility) */
export type BaseValueType =
  | 'reference'
  | 'immediate'
  | 'composite'
  | 'template';

interface ValueTypeOption {
  value: ValueType;
  label: string;
  description: string;
  /** Icon name from lucide-react */
  icon: 'link' | 'pin' | 'braces' | 'list' | 'code';
  /** Badge color class */
  badgeColor: string;
}

export const VALUE_TYPE_OPTIONS: ValueTypeOption[] = [
  {
    value: 'reference',
    label: 'Reference',
    description: 'Reference to data path (e.g., steps.step1.outputs.result)',
    icon: 'link',
    badgeColor: 'bg-blue-100 text-blue-700 border-blue-200',
  },
  {
    value: 'immediate',
    label: 'Immediate',
    description: 'Literal value (string, number, boolean)',
    icon: 'pin',
    badgeColor: 'bg-gray-100 text-gray-700 border-gray-200',
  },
  {
    value: 'composite-object',
    label: 'Composite Object',
    description: 'Build a nested object with mixed value types',
    icon: 'braces',
    badgeColor: 'bg-green-100 text-green-700 border-green-200',
  },
  {
    value: 'composite-array',
    label: 'Composite Array',
    description: 'Build an array with mixed value types',
    icon: 'list',
    badgeColor: 'bg-green-100 text-green-700 border-green-200',
  },
  {
    value: 'template',
    label: 'Template',
    description: 'Minijinja template with {{ variable }} interpolation',
    icon: 'code',
    badgeColor: 'bg-purple-100 text-purple-700 border-purple-200',
  },
];

/** Get the badge color for a value type */
export function getValueTypeBadgeColor(
  type: ValueType | BaseValueType
): string {
  if (type === 'composite')
    return 'bg-green-100 text-green-700 border-green-200';
  if (type === 'template')
    return 'bg-purple-100 text-purple-700 border-purple-200';
  const option = VALUE_TYPE_OPTIONS.find((opt) => opt.value === type);
  return option?.badgeColor ?? 'bg-gray-100 text-gray-700 border-gray-200';
}
