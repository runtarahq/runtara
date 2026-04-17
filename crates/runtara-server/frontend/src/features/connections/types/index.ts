// Connections types - uses generated API types
import {
  ConnectionDto,
  ConnectionTypeDto,
  CreateConnectionRequest,
  UpdateConnectionRequest,
} from '@/generated/RuntaraRuntimeApi';

// Re-export generated types for convenience
export type {
  ConnectionDto,
  ConnectionTypeDto,
  CreateConnectionRequest,
  UpdateConnectionRequest,
};

// Extended connection type with enriched connection type data
export interface EnrichedConnection extends ConnectionDto {
  connectionType: ConnectionTypeDto | null;
}
