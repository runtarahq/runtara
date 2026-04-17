import { HardDrive } from 'lucide-react';
import { FormSection } from './FormSection';
import { CheckboxInput } from '@/shared/components/checkbox-input';

export function DefaultFileStorageSection() {
  return (
    <FormSection title="File Storage" icon={HardDrive} optional>
      <div className="space-y-2">
        <CheckboxInput
          name="isDefaultFileStorage"
          label="Use as default file storage"
        />
        <p className="text-xs text-slate-500 dark:text-slate-400">
          When enabled, incoming webhook attachments (e.g. from Mailgun, Slack)
          will be stored in this connection. Only one connection can be the
          default.
        </p>
      </div>
    </FormSection>
  );
}
