import { Navigate, useLocation } from 'react-router';

/** Keep bookmarks and existing run links, including their filters, working. */
export function InvocationHistoryRedirect() {
  const { search, hash } = useLocation();
  return (
    <Navigate to={{ pathname: '/operations/runs', search, hash }} replace />
  );
}
