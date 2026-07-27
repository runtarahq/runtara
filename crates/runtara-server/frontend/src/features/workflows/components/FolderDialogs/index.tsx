import { useState, useEffect } from 'react';
import { Folder, FolderPlus, Pencil, Trash2, Home } from 'lucide-react';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/shared/components/ui/dialog';
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/shared/components/ui/alert-dialog';
import { Button } from '@/shared/components/ui/button';
import { Input } from '@/shared/components/ui/input';
import { Label } from '@/shared/components/ui/label';
import { cn } from '@/lib/utils';
import { FolderInfo, getFolderName } from '../../hooks/useFolders';
import { Spinner } from '@/shared/components/ui/spinner';

// ==================== Rename Folder Dialog ====================

interface RenameFolderDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onConfirm: (currentPath: string, newName: string) => void;
  folderPath: string;
  isLoading?: boolean;
}

export function RenameFolderDialog({
  open,
  onOpenChange,
  onConfirm,
  folderPath,
  isLoading = false,
}: RenameFolderDialogProps) {
  const currentName = getFolderName(folderPath);
  const [newName, setNewName] = useState(currentName);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (open) {
      setNewName(currentName);
      setError(null);
    }
  }, [open, currentName]);

  const handleConfirm = () => {
    const trimmedName = newName.trim();
    if (!trimmedName) {
      setError('Folder name is required');
      return;
    }
    if (trimmedName.includes('/')) {
      setError('Folder name cannot contain "/"');
      return;
    }
    if (trimmedName === currentName) {
      onOpenChange(false);
      return;
    }
    onConfirm(folderPath, trimmedName);
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <Pencil className="size-5 text-primary" />
            Rename folder
          </DialogTitle>
          <DialogDescription>
            Rename "{currentName}" to a new name.
          </DialogDescription>
        </DialogHeader>
        <div className="space-y-4 py-4">
          <div className="space-y-2">
            <Label htmlFor="newFolderName">New name</Label>
            <Input
              id="newFolderName"
              value={newName}
              onChange={(e) => {
                setNewName(e.target.value);
                setError(null);
              }}
              placeholder="Enter new folder name"
              className={error ? 'border-destructive' : ''}
              onKeyDown={(e) => {
                if (e.key === 'Enter' && !isLoading) {
                  handleConfirm();
                }
              }}
              autoFocus
            />
            {error && <p className="text-xs text-destructive">{error}</p>}
          </div>
        </div>
        <DialogFooter>
          <Button
            variant="secondary"
            onClick={() => onOpenChange(false)}
            disabled={isLoading}
          >
            Cancel
          </Button>
          <Button
            onClick={handleConfirm}
            disabled={isLoading || !newName.trim()}
          >
            {isLoading ? (
              <>
                <Spinner className="mr-2 size-4" />
                Renaming...
              </>
            ) : (
              'Rename'
            )}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

// ==================== Delete Folder Confirmation ====================

interface DeleteFolderDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onConfirm: (path: string) => void;
  folderPath: string;
  workflowCount: number;
  isLoading?: boolean;
}

export function DeleteFolderDialog({
  open,
  onOpenChange,
  onConfirm,
  folderPath,
  workflowCount,
  isLoading = false,
}: DeleteFolderDialogProps) {
  const folderName = getFolderName(folderPath);

  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle className="flex items-center gap-2">
            <Trash2 className="size-5 text-destructive" />
            Delete folder "{folderName}"?
          </AlertDialogTitle>
          <AlertDialogDescription>
            {workflowCount > 0 ? (
              <>
                This folder contains {workflowCount} workflow
                {workflowCount !== 1 ? 's' : ''}. Deleting the folder will move
                all workflows to the root level. This action cannot be undone.
              </>
            ) : (
              'This folder is empty. Deleting it cannot be undone.'
            )}
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel disabled={isLoading}>Cancel</AlertDialogCancel>
          <AlertDialogAction
            onClick={() => onConfirm(folderPath)}
            disabled={isLoading}
            className="bg-destructive hover:bg-destructive/90 focus-visible:ring-destructive"
          >
            {isLoading ? (
              <>
                <Spinner className="mr-2 size-4" />
                Deleting...
              </>
            ) : (
              'Delete folder'
            )}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}

// ==================== Move to Folder Dialog ====================

interface MoveToFolderDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onConfirm: (targetPath: string) => void;
  workflowName: string;
  currentPath?: string;
  folders: FolderInfo[];
  isLoading?: boolean;
}

export function MoveToFolderDialog({
  open,
  onOpenChange,
  onConfirm,
  workflowName,
  currentPath = '/',
  folders,
  isLoading = false,
}: MoveToFolderDialogProps) {
  const [selectedPath, setSelectedPath] = useState<string>('/');
  const [isCreatingNew, setIsCreatingNew] = useState(false);
  const [newFolderName, setNewFolderName] = useState('');
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (open) {
      // Default to root if not already in root
      setSelectedPath(currentPath === '/' ? '/' : '/');
      setIsCreatingNew(false);
      setNewFolderName('');
      setError(null);
    }
  }, [open, currentPath]);

  const handleConfirm = () => {
    if (isCreatingNew) {
      const trimmedName = newFolderName.trim();
      if (!trimmedName) {
        setError('Folder name is required');
        return;
      }
      if (trimmedName.includes('/')) {
        setError('Folder name cannot contain "/"');
        return;
      }
      // Create path for new folder
      const newPath = `/${trimmedName}/`;
      onConfirm(newPath);
      return;
    }

    if (selectedPath === currentPath) {
      onOpenChange(false);
      return;
    }
    onConfirm(selectedPath);
  };

  // Filter out current folder from options
  const availableFolders = folders.filter((f) => f.path !== currentPath);

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <Folder className="size-5 text-amber-500" />
            Move workflow
          </DialogTitle>
          <DialogDescription>
            Move "{workflowName}" to a different folder.
          </DialogDescription>
        </DialogHeader>
        <div className="max-h-[300px] space-y-2 overflow-y-auto py-4">
          {/* Create new folder option */}
          <button
            onClick={() => {
              setIsCreatingNew(true);
              setSelectedPath('');
              setError(null);
            }}
            className={cn(
              'flex w-full items-center gap-3 rounded-lg px-3 py-2.5 text-left transition-colors',
              isCreatingNew
                ? 'border border-amber-200 bg-amber-50 dark:border-amber-700 dark:bg-amber-900/30'
                : 'border border-dashed border-input hover:bg-muted'
            )}
          >
            <FolderPlus
              className={cn(
                'size-5',
                isCreatingNew ? 'text-amber-600' : 'text-amber-500'
              )}
            />
            <p
              className={cn(
                'text-sm font-medium',
                isCreatingNew ? 'text-amber-600' : 'text-foreground'
              )}
            >
              Create new folder
            </p>
          </button>

          {/* New folder input */}
          {isCreatingNew && (
            <div className="px-3 py-2">
              <Input
                value={newFolderName}
                onChange={(e) => {
                  setNewFolderName(e.target.value);
                  setError(null);
                }}
                placeholder="Enter folder name..."
                className={cn('h-9', error ? 'border-destructive' : '')}
                autoFocus
                onKeyDown={(e) => {
                  if (e.key === 'Enter' && !isLoading) {
                    handleConfirm();
                  }
                  if (e.key === 'Escape') {
                    setIsCreatingNew(false);
                    setNewFolderName('');
                    setError(null);
                  }
                }}
              />
              {error && (
                <p className="mt-1 text-xs text-destructive">{error}</p>
              )}
            </div>
          )}

          <div className="my-2 border-t border-border" />

          {/* Root option */}
          <button
            onClick={() => {
              setSelectedPath('/');
              setIsCreatingNew(false);
              setNewFolderName('');
              setError(null);
            }}
            className={cn(
              'flex w-full items-center gap-3 rounded-lg px-3 py-2.5 text-left transition-colors',
              selectedPath === '/' && !isCreatingNew
                ? 'border border-primary/30 bg-primary/10'
                : 'hover:bg-muted'
            )}
          >
            <Home
              className={cn(
                'size-5',
                selectedPath === '/' && !isCreatingNew
                  ? 'text-primary'
                  : 'text-muted-foreground'
              )}
            />
            <div>
              <p
                className={cn(
                  'text-sm font-medium',
                  selectedPath === '/' && !isCreatingNew
                    ? 'text-primary'
                    : 'text-foreground'
                )}
              >
                Root (All Workflows)
              </p>
              <p className="text-xs text-muted-foreground">No folder</p>
            </div>
            {currentPath === '/' && (
              <span className="ml-auto text-xs text-muted-foreground">
                Current
              </span>
            )}
          </button>

          {/* Folder options */}
          {availableFolders.map((folder) => (
            <button
              key={folder.path}
              onClick={() => {
                setSelectedPath(folder.path);
                setIsCreatingNew(false);
                setNewFolderName('');
                setError(null);
              }}
              className={cn(
                'flex w-full items-center gap-3 rounded-lg px-3 py-2.5 text-left transition-colors',
                selectedPath === folder.path && !isCreatingNew
                  ? 'border border-primary/30 bg-primary/10'
                  : 'hover:bg-muted'
              )}
              style={{ paddingLeft: `${12 + (folder.depth - 1) * 16}px` }}
            >
              <Folder
                className={cn(
                  'size-5',
                  selectedPath === folder.path && !isCreatingNew
                    ? 'text-primary'
                    : 'text-amber-500'
                )}
              />
              <div>
                <p
                  className={cn(
                    'text-sm font-medium',
                    selectedPath === folder.path && !isCreatingNew
                      ? 'text-primary'
                      : 'text-foreground'
                  )}
                >
                  {folder.name}
                </p>
                <p className="text-xs text-muted-foreground">{folder.path}</p>
              </div>
              {folder.path === currentPath && (
                <span className="ml-auto text-xs text-muted-foreground">
                  Current
                </span>
              )}
            </button>
          ))}
        </div>
        <DialogFooter>
          <Button
            variant="secondary"
            onClick={() => onOpenChange(false)}
            disabled={isLoading}
          >
            Cancel
          </Button>
          <Button
            onClick={handleConfirm}
            disabled={
              isLoading ||
              (!isCreatingNew && selectedPath === currentPath) ||
              (isCreatingNew && !newFolderName.trim())
            }
          >
            {isLoading ? (
              <>
                <Spinner className="mr-2 size-4" />
                Moving...
              </>
            ) : isCreatingNew ? (
              <>
                <FolderPlus className="mr-2 size-4" />
                Create & Move
              </>
            ) : (
              'Move here'
            )}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
