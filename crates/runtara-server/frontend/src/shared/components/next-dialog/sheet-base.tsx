import { Loader2 } from '../loader.tsx';

type Props = {
  title?: string;
  loading: boolean;
  contentScrollable?: boolean;
  children: React.ReactNode;
};

export function SheetBase(props: Props) {
  const { children, title, loading, contentScrollable = true } = props;

  return (
    <div className="flex h-full min-h-0 flex-col">
      {/* Header - only show if title provided */}
      {title && (
        <div className="shrink-0 pb-3">
          <h2 className="text-lg font-semibold text-slate-900/90 dark:text-slate-100">
            {title}
          </h2>
        </div>
      )}

      {/* Content area - scrollable */}
      <div
        className={`min-h-0 flex-1 space-y-4 ${
          contentScrollable ? 'overflow-y-auto' : ''
        }`}
      >
        {loading ? <Loader2 /> : children}
      </div>
    </div>
  );
}
