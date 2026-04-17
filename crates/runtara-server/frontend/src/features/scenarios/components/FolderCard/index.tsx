import { CSSProperties, useState } from 'react';
import {
  Folder,
  FolderOpen,
  ChevronRight,
  Pencil,
  Trash2,
  MoreHorizontal,
  Clock,
} from 'lucide-react';
import { cn } from '@/lib/utils';
import { Button } from '@/shared/components/ui/button';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/shared/components/ui/dropdown-menu';

export interface FolderCardProps {
  /** Folder display name */
  name: string;
  /** Full folder path */
  path: string;
  /** Number of scenarios in this folder */
  scenarioCount: number;
  /** Relative time since last update */
  updatedAgo?: string;
  /** Called when folder is clicked to open */
  onOpen: (path: string) => void;
  /** Called when rename is requested */
  onRename?: (path: string) => void;
  /** Called when delete is requested */
  onDelete?: (path: string) => void;
  /** Custom className */
  className?: string;
  /** Custom style */
  style?: CSSProperties;
  /** Whether actions are disabled (e.g., during mutation) */
  disabled?: boolean;
}

export function FolderCard({
  name,
  path,
  scenarioCount,
  updatedAgo,
  onOpen,
  onRename,
  onDelete,
  className,
  style,
  disabled = false,
}: FolderCardProps) {
  const [isHovered, setIsHovered] = useState(false);

  return (
    <div
      className={cn(
        'group relative bg-white rounded-xl border transition-all duration-200 ease-out cursor-pointer',
        isHovered
          ? 'border-amber-300 shadow-lg shadow-amber-100/50 bg-amber-50/30 dark:border-amber-600 dark:shadow-amber-900/20 dark:bg-amber-900/10'
          : 'border-slate-200/80 hover:border-slate-300 dark:border-slate-700/80 dark:hover:border-slate-600',
        disabled && 'opacity-60 pointer-events-none',
        className
      )}
      style={style}
      onMouseEnter={() => setIsHovered(true)}
      onMouseLeave={() => setIsHovered(false)}
      onClick={() => onOpen(path)}
    >
      {/* Colored left accent */}
      <div className="absolute left-0 top-4 bottom-4 w-1 rounded-full bg-amber-400 dark:bg-amber-500" />

      <div className="p-5 pl-6">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-3 min-w-0">
            <div
              className={cn(
                'w-10 h-10 rounded-lg flex items-center justify-center flex-shrink-0 transition-colors',
                isHovered
                  ? 'bg-amber-100 dark:bg-amber-900/40'
                  : 'bg-amber-50 dark:bg-amber-900/20'
              )}
            >
              {isHovered ? (
                <FolderOpen className="w-5 h-5 text-amber-600 dark:text-amber-400" />
              ) : (
                <Folder className="w-5 h-5 text-amber-500 dark:text-amber-400" />
              )}
            </div>
            <div className="min-w-0">
              <h3 className="text-[15px] font-semibold text-slate-900 dark:text-slate-100 flex items-center gap-2 truncate">
                <span className="truncate">{name}</span>
                <ChevronRight
                  className={cn(
                    'w-4 h-4 text-slate-400 flex-shrink-0 transition-transform dark:text-slate-500',
                    isHovered && 'translate-x-1'
                  )}
                />
              </h3>
              <div className="flex items-center gap-3 mt-0.5">
                <span className="text-xs text-slate-500 dark:text-slate-400">
                  {scenarioCount} scenario{scenarioCount !== 1 ? 's' : ''}
                </span>
                {updatedAgo && (
                  <span className="text-xs text-slate-400 dark:text-slate-500 flex items-center gap-1">
                    <Clock className="w-3 h-3" />
                    {updatedAgo}
                  </span>
                )}
              </div>
            </div>
          </div>

          {/* Actions */}
          <div
            className={cn(
              'flex items-center gap-1 transition-opacity duration-150',
              isHovered ? 'opacity-100' : 'opacity-0'
            )}
            onClick={(e) => e.stopPropagation()}
          >
            {onRename && (
              <Button
                variant="ghost"
                size="icon"
                onClick={() => onRename(path)}
                title="Rename"
                className="p-2 h-auto w-auto text-slate-400 hover:text-blue-600 hover:bg-blue-50 dark:hover:bg-blue-900/30 dark:hover:text-blue-400 rounded-lg transition-colors"
              >
                <Pencil className="w-4 h-4" />
              </Button>
            )}
            {onDelete && (
              <Button
                variant="ghost"
                size="icon"
                onClick={() => onDelete(path)}
                title="Delete"
                className="p-2 h-auto w-auto text-slate-400 hover:text-red-600 hover:bg-red-50 dark:hover:bg-red-900/30 dark:hover:text-red-400 rounded-lg transition-colors"
              >
                <Trash2 className="w-4 h-4" />
              </Button>
            )}
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button
                  variant="ghost"
                  size="icon"
                  title="More options"
                  className="p-2 h-auto w-auto text-slate-400 hover:text-slate-600 hover:bg-slate-100 dark:hover:bg-slate-800 dark:hover:text-slate-300 rounded-lg transition-colors"
                >
                  <MoreHorizontal className="w-4 h-4" />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                {onRename && (
                  <DropdownMenuItem onClick={() => onRename(path)}>
                    <Pencil className="w-4 h-4 mr-2" />
                    Rename folder
                  </DropdownMenuItem>
                )}
                {onDelete && (
                  <DropdownMenuItem
                    onClick={() => onDelete(path)}
                    className="text-red-600 focus:text-red-600"
                  >
                    <Trash2 className="w-4 h-4 mr-2" />
                    Delete folder
                  </DropdownMenuItem>
                )}
              </DropdownMenuContent>
            </DropdownMenu>
          </div>
        </div>
      </div>
    </div>
  );
}
