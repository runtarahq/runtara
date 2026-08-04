import { FileQuestion } from 'lucide-react';
import { Link, useLocation, useNavigate } from 'react-router';
import { Button } from '@/shared/components/ui/button';

/**
 * Catch-all screen for URLs that match no route — a typo, a stale bookmark, or
 * a link to something that has since been deleted. Mounted by the router's
 * wildcard route.
 *
 * Deliberately outside `<PrivateRoute>`: a URL that doesn't exist should say so
 * rather than send the visitor through a login round-trip that lands them
 * somewhere else.
 *
 * Two ways out, because neither covers every case: "Back to workflows" is the
 * reliable one, and "Go back" is the right one when the bad link came from
 * somewhere inside the app.
 */
export function NotFound() {
  const { pathname } = useLocation();
  const navigate = useNavigate();

  return (
    <section
      role="region"
      aria-labelledby="not-found-heading"
      className="flex min-h-[60vh] flex-col items-center justify-center px-6 text-center"
    >
      <FileQuestion
        className="mb-4 size-12 text-muted-foreground"
        aria-hidden="true"
      />
      <h2 id="not-found-heading" className="mb-2 text-2xl font-semibold">
        Page not found
      </h2>
      <p className="mb-2 max-w-md text-muted-foreground">
        There&apos;s nothing at this address. The link may be out of date, or
        the page may have been removed.
      </p>
      <code className="mb-6 max-w-full truncate rounded bg-muted px-2 py-1 text-sm text-muted-foreground">
        {pathname}
      </code>
      <div className="flex flex-wrap items-center justify-center gap-3">
        <Button asChild>
          <Link to="/workflows">Back to workflows</Link>
        </Button>
        <Button variant="secondary" bordered onClick={() => navigate(-1)}>
          Go back
        </Button>
      </div>
    </section>
  );
}
