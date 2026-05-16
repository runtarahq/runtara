import { useNavigate } from 'react-router';
import { ObjectSchemaDtoForm } from '@/features/objects/components/ObjectSchemaForm';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { ObjectModelConnectionSelector } from '@/features/objects/components/ObjectModelConnectionSelector';
import { useObjectModelConnectionSelection } from '@/features/objects/hooks/useObjectModelConnectionSelection';

export function CreateObjectSchema() {
  const navigate = useNavigate();
  usePageTitle('Create Object Type');
  const { selectedConnectionId, connectionQuery } =
    useObjectModelConnectionSelection();

  const handleSuccess = () => {
    navigate(`/objects/types${connectionQuery}`);
  };

  return (
    <div className="space-y-4">
      <div className="flex justify-end px-4 pt-4 sm:px-6 lg:px-10">
        <ObjectModelConnectionSelector />
      </div>
      <ObjectSchemaDtoForm
        onSuccess={handleSuccess}
        connectionId={selectedConnectionId}
      />
    </div>
  );
}
