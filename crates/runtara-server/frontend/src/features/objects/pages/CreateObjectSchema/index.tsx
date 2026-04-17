import { useNavigate } from 'react-router';
import { ObjectSchemaDtoForm } from '@/features/objects/components/ObjectSchemaForm';
import { usePageTitle } from '@/shared/hooks/usePageTitle';

export function CreateObjectSchema() {
  const navigate = useNavigate();
  usePageTitle('Create Object Type');

  const handleSuccess = () => {
    navigate('/objects/types');
  };

  return <ObjectSchemaDtoForm onSuccess={handleSuccess} />;
}
