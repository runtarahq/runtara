import { useState } from 'react';
import { useNavigate } from 'react-router';
import { ExistingConnections } from '@/features/connections/components/ExistingConnections';
import { ConnectionPickerModal } from '@/features/connections/components/ConnectionPickerModal';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { Button } from '@/shared/components/ui/button';
import { Plus, Loader2 } from 'lucide-react';
import { useCustomQuery } from '@/shared/hooks/api';
import { queryKeys } from '@/shared/queries/query-keys';
import {
  getConnectionTypes,
  getConnections,
} from '@/features/connections/queries';
import { ConnectionTypeDto } from '@/generated/RuntaraRuntimeApi';
import { TilesPage } from '@/shared/components/tiles-page';

export function Connections() {
  const navigate = useNavigate();
  const [isModalOpen, setIsModalOpen] = useState(false);

  const {
    data: connectionTypes = [],
    isFetching,
    isError: connectionTypesError,
  } = useCustomQuery({
    queryKey: queryKeys.connections.types(),
    queryFn: getConnectionTypes,
  });

  const { isError: connectionsError } = useCustomQuery({
    queryKey: queryKeys.connections.all,
    queryFn: getConnections,
  });

  const handleCreate = (connectionType: ConnectionTypeDto) => {
    if (!connectionType?.integrationId) return;
    navigate(`/connections/${connectionType.integrationId}/create`);
  };

  usePageTitle('Connections');
  return (
    <>
      <TilesPage
        kicker="Connections"
        title="Manage connections"
        action={
          <Button
            className="h-11 w-full rounded-full sm:w-auto sm:px-6"
            disabled={
              isFetching ||
              connectionTypes.length === 0 ||
              connectionTypesError ||
              connectionsError
            }
            onClick={() => setIsModalOpen(true)}
          >
            {isFetching ? (
              <>
                <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                Loading...
              </>
            ) : (
              <>
                <Plus className="mr-2 h-4 w-4" />
                New connection
              </>
            )}
          </Button>
        }
      >
        <ExistingConnections />
      </TilesPage>

      <ConnectionPickerModal
        open={isModalOpen}
        onOpenChange={setIsModalOpen}
        onSelect={handleCreate}
        connectionTypes={connectionTypes}
        isLoading={isFetching}
      />
    </>
  );
}
