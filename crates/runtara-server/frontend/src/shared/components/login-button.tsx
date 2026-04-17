import { useAuth } from 'react-oidc-context';
import { LogInIcon } from 'lucide-react';
import { Button } from '@/shared/components/ui/button.tsx';

export default function LoginButton() {
  const { signinRedirect } = useAuth();

  return (
    <Button className="w-full" onClick={() => signinRedirect()}>
      <LogInIcon size={16} />
      Sign in
    </Button>
  );
}
