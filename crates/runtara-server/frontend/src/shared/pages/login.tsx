import AuthButton from '@/shared/components/auth-button.tsx';
import { useAuth } from 'react-oidc-context';

export function Login() {
  const { isAuthenticated } = useAuth();

  const message = isAuthenticated ? (
    'You are already signed in.'
  ) : (
    <>
      <b>Sign in now</b> to explore all our integration options and get started.
    </>
  );

  return (
    <section className="flex h-[calc(100vh-300px)] flex-col items-center justify-center">
      <div className="flex flex-col items-start">
        <h2 className="mb-6 max-w-96 font-light">{message}</h2>
        <AuthButton />
      </div>
    </section>
  );
}
