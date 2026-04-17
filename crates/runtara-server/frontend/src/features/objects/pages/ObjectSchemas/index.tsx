import { useNavigate } from 'react-router';
import { Button } from '@/shared/components/ui/button';
import { ObjectSchemaDtosTable } from '@/features/objects/components/ObjectSchemasTable';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { TilesPage, TileList } from '@/shared/components/tiles-page';
import { useObjectSchemaDtos } from '@/features/objects/hooks/useObjectSchemas';

export function ObjectSchemas() {
  const navigate = useNavigate();
  usePageTitle('Object Types');
  const { isError } = useObjectSchemaDtos();

  return (
    <TilesPage
      kicker="Database"
      title="Object types"
      action={
        <Button
          className="h-11 w-full rounded-full sm:w-auto sm:px-6"
          onClick={() => navigate('/objects/types/create')}
          disabled={isError}
        >
          Create object type
        </Button>
      }
    >
      <TileList>
        <ObjectSchemaDtosTable />
      </TileList>
    </TilesPage>
  );
}
