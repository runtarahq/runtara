import { CSSProperties, ReactNode } from 'react';
import { cn } from '@/lib/utils';

type EntityTileProps = {
  kicker?: string | ReactNode;
  title: string | ReactNode;
  badges?: ReactNode;
  description?: string | ReactNode;
  metadata?: Array<string | ReactNode>;
  tags?: ReactNode;
  actions?: ReactNode;
  footer?: ReactNode;
  showDivider?: boolean;
  className?: string;
  style?: CSSProperties;
  contentPaddingClassName?: string;
};

export function EntityTile(props: EntityTileProps) {
  const {
    kicker,
    title,
    badges,
    description,
    metadata = [],
    tags,
    actions,
    footer,
    showDivider = false,
    className,
    style,
    contentPaddingClassName = 'p-5',
  } = props;

  const hasMetadata = metadata.length > 0;

  return (
    <article
      className={cn(
        'group relative rounded-xl bg-white border border-slate-200/80 hover:border-slate-300 hover:shadow-lg hover:shadow-slate-200/50 transition-all duration-200 ease-out dark:bg-card dark:border-slate-700/50 dark:hover:border-slate-600 dark:hover:shadow-slate-900/30',
        contentPaddingClassName,
        className
      )}
      style={style}
    >
      <div className="flex flex-col gap-2">
        {/* Header row with title, badges, and actions */}
        <div className="flex items-start justify-between gap-2 sm:gap-3">
          <div className="flex items-center gap-2 min-w-0 flex-1">
            {kicker && (
              <span className="flex-shrink-0 px-1.5 py-0.5 text-[10px] font-medium text-slate-500 bg-slate-100 rounded-md dark:bg-slate-800 dark:text-slate-400">
                {kicker}
              </span>
            )}
            <h3 className="text-[15px] font-semibold text-slate-900 dark:text-slate-100 truncate min-w-0">
              {title}
            </h3>
            {badges && (
              <div className="flex-shrink-0 hidden sm:flex">{badges}</div>
            )}
          </div>

          {/* Desktop: show on hover */}
          {actions && (
            <div className="hidden sm:flex items-center gap-1 flex-shrink-0 opacity-0 group-hover:opacity-100 transition-opacity duration-150">
              {actions}
            </div>
          )}
        </div>

        {/* Mobile: show actions below title */}
        {actions && (
          <div className="flex sm:hidden items-center gap-1 -ml-1">
            {actions}
          </div>
        )}

        {/* Description */}
        {description && (
          <p className="text-sm text-slate-500 leading-relaxed line-clamp-2 dark:text-slate-400">
            {description}
          </p>
        )}

        {/* Metadata row */}
        {(hasMetadata || tags) && (
          <div className="flex flex-col sm:flex-row sm:items-center gap-2 sm:gap-4 text-xs text-slate-400 dark:text-slate-500 min-w-0 overflow-hidden">
            {hasMetadata && (
              <div className="flex items-center gap-4 flex-shrink-0">
                {metadata.map((item, index) => (
                  <span
                    key={`${index}-${String(item)}`}
                    className="inline-flex items-center gap-1.5 whitespace-nowrap"
                  >
                    {item}
                  </span>
                ))}
              </div>
            )}
            {tags && <div className="min-w-0 overflow-hidden">{tags}</div>}
          </div>
        )}
      </div>

      {footer && (
        <div
          className={cn(
            'mt-4 pt-4',
            showDivider &&
              'border-t border-slate-200/60 dark:border-slate-700/40'
          )}
        >
          {footer}
        </div>
      )}
    </article>
  );
}
