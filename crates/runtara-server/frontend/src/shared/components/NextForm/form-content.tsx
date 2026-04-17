import { FormField } from './form-field.tsx';
import { cn } from '@/lib/utils.ts';
import { Fragment } from 'react';

type Props = {
  className?: string;
  fieldsConfig: any;
};

export function FormContent(props: Props) {
  const { className, fieldsConfig } = props;
  const hasCustomGridColumns =
    typeof className === 'string' && className.includes('grid-cols');
  return (
    <div
      className={cn(
        'grid gap-3 py-3',
        !hasCustomGridColumns && 'grid-cols-2',
        className
      )}
    >
      {fieldsConfig.map((config: any, index: number) => {
        const { label, name, renderComponent, renderFormField, colSpan, type } =
          config;

        const key = name || label || `field-${index}`;

        // Skip hidden fields completely (don't render container div)
        if (type === 'hidden') {
          return (
            <Fragment key={key}>
              {renderFormField ? (
                renderFormField(config)
              ) : (
                <FormField {...config} />
              )}
            </Fragment>
          );
        }

        // Render the field content first to check if it's null or undefined
        let fieldContent;
        if (renderComponent) {
          fieldContent = renderComponent(config);
        } else if (renderFormField) {
          fieldContent = renderFormField(config);
        } else {
          fieldContent = <FormField {...config} />;
        }

        // Skip rendering the container if field content is null or undefined
        if (!fieldContent) {
          return null;
        }

        // Support various colSpan values for flexible grid layouts
        let colSpanClass = 'col-span-1';
        if (colSpan === 'full' || colSpan === 'all') {
          colSpanClass = 'col-span-full';
        } else if (colSpan === 2 || colSpan === '2') {
          colSpanClass = 'col-span-2';
        } else if (colSpan === 3 || colSpan === '3') {
          colSpanClass = 'col-span-3';
        } else if (colSpan === '1') {
          colSpanClass = 'col-span-1';
        }
        return (
          <div key={key} className={colSpanClass}>
            {fieldContent}
          </div>
        );
      })}
    </div>
  );
}
