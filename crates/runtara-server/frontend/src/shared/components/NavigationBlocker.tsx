import { useEffect, useCallback } from 'react';
import { useBlocker, type Location } from 'react-router';
import { useNavigationBlockerStore } from '@/shared/stores/navigationBlockerStore';
import {
  AlertDialog,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogCancel,
  AlertDialogAction,
} from '@/shared/components/ui/alert-dialog';

export function NavigationBlocker() {
  const {
    shouldBlock,
    isDialogOpen,
    setBlockerFunctions,
    openDialog,
    confirmNavigation,
    cancelNavigation,
  } = useNavigationBlockerStore();

  // Use the blocker at the layout level where it won't be unmounted
  const blocker = useBlocker(
    useCallback(
      ({
        currentLocation,
        nextLocation,
      }: {
        currentLocation: Location;
        nextLocation: Location;
      }) => {
        const shouldBlockNav =
          shouldBlock && currentLocation.pathname !== nextLocation.pathname;
        console.log(
          '[NavigationBlocker] shouldBlock:',
          shouldBlockNav,
          'shouldBlock state:',
          shouldBlock
        );
        return shouldBlockNav;
      },
      [shouldBlock]
    )
  );

  // When blocker becomes blocked, store the proceed/reset functions and show dialog
  useEffect(() => {
    console.log('[NavigationBlocker] blocker.state:', blocker.state);
    if (blocker.state === 'blocked') {
      console.log('[NavigationBlocker] Navigation blocked, opening dialog');
      setBlockerFunctions(blocker.proceed, blocker.reset);
      openDialog();
    }
  }, [
    blocker.state,
    blocker.proceed,
    blocker.reset,
    setBlockerFunctions,
    openDialog,
  ]);

  return (
    <AlertDialog
      open={isDialogOpen}
      onOpenChange={(open) => !open && cancelNavigation()}
    >
      <AlertDialogContent className="p-6">
        <AlertDialogHeader className="pb-4">
          <AlertDialogTitle>Unsaved Changes</AlertDialogTitle>
          <AlertDialogDescription className="pt-2">
            You have unsaved changes that will be lost. Are you sure you want to
            continue?
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter className="pt-4">
          <AlertDialogCancel onClick={cancelNavigation}>
            Cancel
          </AlertDialogCancel>
          <AlertDialogAction
            onClick={confirmNavigation}
            className="bg-orange-600 hover:bg-orange-700"
          >
            Discard Changes
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
