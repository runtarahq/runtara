import { useAuth } from 'react-oidc-context';
import LogoutButton from '@/shared/components/logout-button.tsx';
import LoginButton from '@/shared/components/login-button.tsx';

export default function AuthButton() {
  const auth = useAuth();

  return auth.isAuthenticated ? <LogoutButton /> : <LoginButton />;
}
