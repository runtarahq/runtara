import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select';

export type DateRangeOption = '1h' | '24h' | '7d' | '30d' | '90d';

interface DateRangeSelectorProps {
  value: DateRangeOption;
  onChange: (value: DateRangeOption) => void;
  options?: DateRangeOption[];
}

const allDateRangeOptions = [
  { label: 'Last Hour', value: '1h' as DateRangeOption },
  { label: 'Last 24 Hours', value: '24h' as DateRangeOption },
  { label: 'Last 7 Days', value: '7d' as DateRangeOption },
  { label: 'Last 30 Days', value: '30d' as DateRangeOption },
  { label: 'Last 90 Days', value: '90d' as DateRangeOption },
];

export function DateRangeSelector({
  value,
  onChange,
  options,
}: DateRangeSelectorProps) {
  const dateRangeOptions = options
    ? allDateRangeOptions.filter((opt) => options.includes(opt.value))
    : allDateRangeOptions;

  return (
    <Select
      value={value}
      onValueChange={(val) => onChange(val as DateRangeOption)}
    >
      <SelectTrigger className="w-[180px]">
        <SelectValue placeholder="Select date range" />
      </SelectTrigger>
      <SelectContent>
        {dateRangeOptions.map((option) => (
          <SelectItem key={option.value} value={option.value}>
            {option.label}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}
