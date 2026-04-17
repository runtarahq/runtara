import { useAuth } from 'react-oidc-context';

/**
 * Hook to get the current user's access token.
 * Extracts the token from the OIDC auth context.
 *
 * @returns The access token string, or empty string if not authenticated
 *
 * @example
 * ```tsx
 * function MyComponent() {
 *   const token = useToken();
 *
 *   const { data } = useQuery({
 *     queryKey: ['myData'],
 *     queryFn: () => fetchData(token),
 *     enabled: !!token,
 *   });
 * }
 * ```
 */
export function useToken(): string {
  const auth = useAuth();
  return auth.user?.access_token || '';
}
