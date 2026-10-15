import { render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import { MappingObjectField } from './MappingObjectField';
import { NodeFormContext } from '../NodeFormContext';

/**
 * The runtime honours a `file` type hint on mapping-object entries (the
 * Error step's context, for one), so the row's type picker has to offer it;
 * otherwise a loaded `file` entry renders with a blank type.
 */
describe('MappingObjectField', () => {
  it('shows a file type hint', async () => {
    const ctx = {
      previousSteps: [],
      inputSchemaFields: [],
      variables: [],
      isInsideSplit: false,
      isInsideWaitScope: false,
      splitItemSchemaFields: [],
      nodeId: 'step-1',
      agents: [],
      workflows: [],
      stepTypes: [],
    };
    render(
      <NodeFormContext.Provider value={ctx as never}>
        <MappingObjectField
          value={{
            attachment: {
              valueType: 'reference',
              value: 'data.document',
              type: 'file',
            },
          }}
          onChange={vi.fn()}
        />
      </NodeFormContext.Provider>
    );

    expect(await screen.findByText('File')).toBeInTheDocument();
  });
});
