import { useState } from 'react';
import { Link, useNavigate } from 'react-router';
import { ExistingConnections } from '@/features/connections/components/ExistingConnections';
import { ConnectionPickerModal } from '@/features/connections/components/ConnectionPickerModal';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { Button } from '@/shared/components/ui/button';
import { Can } from '@/shared/components/Can';
import { Gauge, Plus } from 'lucide-react';
import { useCustomQuery } from '@/shared/hooks/api';
import { queryKeys } from '@/shared/queries/query-keys';
import {
  getConnectionTypes,
  getConnections,
} from '@/features/connections/queries';
import { ConnectionTypeDto } from '@/generated/RuntaraRuntimeApi';
import { Breadcrumb, ConsoleToolbar } from '@/shared/components/console';
import { Spinner } from '@/shared/components/ui/spinner';

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

  const toolbar = (
    <ConsoleToolbar
      left={<Breadcrumb items={[{ label: 'Connections' }]} />}
      actions={
        <div className="flex items-center gap-2">
          <Button asChild variant="secondary" bordered>
            <Link to="/connections/rate-limits">
              <Gauge className="mr-2 size-4" />
              Rate limits
            </Link>
          </Button>
          <Can permission="connection:create">
            <Button
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
                  <Spinner className="mr-2 size-4" />
                  Loading...
                </>
              ) : (
                <>
                  <Plus className="mr-2 size-4" />
                  New connection
                </>
              )}
            </Button>
          </Can>
        </div>
      }
    />
  );

  return (
    <>
      <ExistingConnections toolbar={toolbar} />

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
