import { forwardRef } from 'react';
import { Handle, HandleProps } from '@xyflow/react';

import { cn } from '@/lib/utils.ts';

export type BaseHandleProps = HandleProps;

export const BaseHandle = forwardRef<HTMLDivElement, BaseHandleProps>(
  ({ className, children, ...props }, ref) => {
    return (
      <Handle
        ref={ref}
        {...props}
        className={cn('!w-2 !h-2 !rounded-full', className)}
        {...props}
      >
        {children}
      </Handle>
    );
  }
);

BaseHandle.displayName = 'BaseHandle';
