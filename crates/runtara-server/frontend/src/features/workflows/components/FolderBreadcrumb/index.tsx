import { Home, ChevronRight, Folder } from 'lucide-react';
import { cn } from '@/lib/utils';

interface FolderBreadcrumbProps {
  /** Current folder path (e.g., "/Sales/Shopify/") */
  currentPath: string;
  /** All available folders for resolving names (unused for now, kept for future use) */
  folders?: string[];
  /** Called when navigating to a folder */
  onNavigate: (path: string) => void;
  /** Additional className */
  className?: string;
}

/**
 * Breadcrumb navigation for folder hierarchy
 */
export function FolderBreadcrumb({
  currentPath,
  // eslint-disable-next-line @typescript-eslint/no-unused-vars
  folders: _folders = [],
  onNavigate,
  className,
}: FolderBreadcrumbProps) {
  // Build breadcrumb trail from current path
  const pathSegments = currentPath
    .replace(/^\/|\/$/g, '')
    .split('/')
    .filter(Boolean);

  // Build cumulative paths for each segment
  const breadcrumbItems: { path: string; name: string }[] = pathSegments.map(
    (segment, index) => ({
      path: '/' + pathSegments.slice(0, index + 1).join('/') + '/',
      name: segment,
    })
  );

  const isAtRoot = currentPath === '/' || currentPath === '';

  return (
    <nav className={cn('flex items-center gap-2 text-sm', className)}>
      {/* Home / All Workflows */}
      <button
        onClick={() => onNavigate('/')}
        className={cn(
          'flex items-center gap-1.5 px-2 py-1 rounded-md transition-colors',
          isAtRoot
            ? 'text-slate-900 font-medium dark:text-slate-100'
            : 'text-slate-500 hover:text-slate-700 hover:bg-slate-100 dark:text-slate-400 dark:hover:text-slate-300 dark:hover:bg-slate-800'
        )}
      >
        <Home className="w-4 h-4" />
        All Workflows
      </button>

      {/* Folder trail */}
      {breadcrumbItems.map((item, index) => {
        const isLast = index === breadcrumbItems.length - 1;

        return (
          <div key={item.path} className="flex items-center gap-2">
            <ChevronRight className="w-4 h-4 text-slate-300 dark:text-slate-600" />
            <button
              onClick={() => onNavigate(item.path)}
              className={cn(
                'flex items-center gap-1.5 px-2 py-1 rounded-md transition-colors',
                isLast
                  ? 'text-slate-900 font-medium dark:text-slate-100'
                  : 'text-slate-500 hover:text-slate-700 hover:bg-slate-100 dark:text-slate-400 dark:hover:text-slate-300 dark:hover:bg-slate-800'
              )}
            >
              <Folder className="w-4 h-4 text-amber-500 dark:text-amber-400" />
              {item.name}
            </button>
          </div>
        );
      })}
    </nav>
  );
}
