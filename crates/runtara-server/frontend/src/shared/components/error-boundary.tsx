import { Link, useRouteError } from 'react-router';
import {
  Card,
  CardContent,
  CardFooter,
  CardHeader,
  CardTitle,
} from '@/shared/components/ui/card.tsx';
import { Button } from '@/shared/components/ui/button.tsx';

const { DEV, PROD } = import.meta.env;

export function ErrorBoundary() {
  const error = useRouteError() as Error;

  return (
    <Card>
      <CardHeader>
        <CardTitle>An error has occurred</CardTitle>
      </CardHeader>
      <CardContent>
        {DEV && (
          <div className="space-y-3">
            <p className="text-xl">
              Please report about this issue to the development team:
            </p>
            <div className="bg-accent rounded-sm space-y-3 break-words p-4">
              <pre className="whitespace-pre-wrap">{error.message}</pre>
              <pre className="whitespace-pre-wrap">{error.stack}</pre>
            </div>
          </div>
        )}
        {PROD && (
          <p>
            This page does not exist or is unavailable. Please try again later.
          </p>
        )}
      </CardContent>
      <CardFooter>
        <Link to={'/'}>
          <Button>Back to Home</Button>
        </Link>
      </CardFooter>
    </Card>
  );
}
