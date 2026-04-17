import { useCallback, useEffect, useRef, useState } from 'react';
import { useController, useWatch } from 'react-hook-form';
import { Button } from '@/shared/components/ui/button';
import { Input } from '@/shared/components/ui/input';
import { FormLabel } from '@/shared/components/ui/form';
import { Plus, Trash2 } from 'lucide-react';

const configurationField = 'configuration';

type KeyValuePair = {
  key: string;
  value: string;
};

export function ConfigurationField(props: any) {
  const { label, disabled } = props;

  const { field: configField } = useController({ name: configurationField });
  const triggerTypeWatch = useWatch({ name: 'triggerType' });
  const applicationNameWatch = useWatch({ name: 'applicationName' });
  const eventTypeWatch = useWatch({ name: 'eventType' });

  const [pairs, setPairs] = useState<KeyValuePair[]>([]);
  const initializedRef = useRef(false);

  // Update the configuration when pairs, applicationName, or eventType change
  const updateConfiguration = useCallback(() => {
    // Always create a configuration object, even if empty
    const configuration: any = {};

    // Always include applicationName and eventType if they exist
    if (applicationNameWatch) {
      configuration['applicationName'] = applicationNameWatch;
    }

    if (eventTypeWatch) {
      configuration['eventType'] = eventTypeWatch;
    }

    // Add all other key/value pairs
    pairs.forEach(({ key, value }) => {
      if (key && key !== 'applicationName' && key !== 'eventType') {
        configuration[key] = value;
      }
    });

    // Always set the configuration object
    configField.onChange(configuration);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pairs, applicationNameWatch, eventTypeWatch]); // Removed configField - it's a stable reference from useController

  // Initialize pairs from existing configuration on mount
  useEffect(() => {
    if (
      !initializedRef.current &&
      configField.value &&
      typeof configField.value === 'object'
    ) {
      const existingPairs: KeyValuePair[] = [];
      Object.entries(configField.value).forEach(([key, value]) => {
        // Skip applicationName and eventType as they are handled separately
        if (key !== 'applicationName' && key !== 'eventType') {
          existingPairs.push({ key, value: value as string });
        }
      });
      setPairs(existingPairs);
      initializedRef.current = true;
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []); // Empty dependency array - only run on mount

  // Update configuration when pairs, applicationName, or eventType change
  useEffect(() => {
    updateConfiguration();
  }, [pairs, applicationNameWatch, eventTypeWatch, updateConfiguration]);

  const addPair = () => {
    const newPairs = [...pairs, { key: '', value: '' }];
    setPairs(newPairs);
  };

  const removePair = (index: number) => {
    const newPairs = [...pairs];
    newPairs.splice(index, 1);
    setPairs(newPairs);
  };

  const updatePair = (index: number, field: 'key' | 'value', value: string) => {
    const newPairs = [...pairs];
    newPairs[index][field] = value;
    setPairs(newPairs);
  };

  if (triggerTypeWatch !== 'APPLICATION') {
    return null;
  }

  return (
    <div className="space-y-3">
      <FormLabel>{label}</FormLabel>
      <div className="space-y-2">
        {pairs.map((pair, index) => (
          <div
            key={index}
            className="grid grid-cols-[1fr_1fr_auto] gap-2 items-center"
          >
            <Input
              placeholder="Key"
              value={pair.key}
              onChange={(e) => updatePair(index, 'key', e.target.value)}
              disabled={disabled}
            />
            <Input
              placeholder="Value"
              value={pair.value}
              onChange={(e) => updatePair(index, 'value', e.target.value)}
              disabled={disabled}
            />
            <Button
              type="button"
              variant="ghost"
              size="icon"
              onClick={() => removePair(index)}
              disabled={disabled}
            >
              <Trash2 className="h-4 w-4" />
            </Button>
          </div>
        ))}
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={addPair}
          disabled={disabled}
          className="mt-2"
        >
          <Plus className="h-4 w-4 mr-2" />
          Add Key/Value Pair
        </Button>
      </div>
    </div>
  );
}
