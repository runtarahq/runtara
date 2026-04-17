import { RuntimeREST } from '@/shared/queries';
import { createAuthHeaders } from '@/shared/queries/utils';

// ============================================================================
// Types
// ============================================================================

export interface BucketDto {
  name: string;
  creationDate: string;
}

export interface FileObjectDto {
  key: string;
  size: number;
  lastModified: string;
  etag: string;
}

/** @lintignore Public DTO for file metadata responses; retained for consumer use. */
export interface FileMetadataDto {
  contentType: string;
  contentLength: number;
  etag: string;
  lastModified: string;
}

export interface ListBucketsResponse {
  buckets: BucketDto[];
}

export interface ListObjectsResponse {
  files: FileObjectDto[];
  count: number;
  nextContinuationToken: string | null;
}

// ============================================================================
// Helpers
// ============================================================================

function appendConnectionId(
  url: string,
  connectionId: string,
  hasQuery = false
): string {
  const sep = hasQuery ? '&' : '?';
  return `${url}${sep}connectionId=${encodeURIComponent(connectionId)}`;
}

// ============================================================================
// Bucket Queries
// ============================================================================

export async function listBuckets(
  token: string,
  connectionId: string
): Promise<BucketDto[]> {
  const url = appendConnectionId('/api/runtime/files/buckets', connectionId);
  const result = await RuntimeREST.instance.get<ListBucketsResponse>(
    url,
    createAuthHeaders(token)
  );
  return result.data.buckets || [];
}

export async function createBucket(
  token: string,
  connectionId: string,
  name: string
): Promise<void> {
  const url = appendConnectionId('/api/runtime/files/buckets', connectionId);
  await RuntimeREST.instance.post(url, { name }, createAuthHeaders(token));
}

export async function deleteBucket(
  token: string,
  connectionId: string,
  name: string
): Promise<void> {
  const url = appendConnectionId(
    `/api/runtime/files/buckets/${encodeURIComponent(name)}`,
    connectionId
  );
  await RuntimeREST.instance.delete(url, createAuthHeaders(token));
}

// ============================================================================
// Object / File Queries
// ============================================================================

export async function listObjects(
  token: string,
  connectionId: string,
  bucket: string,
  prefix?: string,
  maxKeys?: number,
  continuationToken?: string
): Promise<ListObjectsResponse> {
  const params = new URLSearchParams();
  params.set('connectionId', connectionId);
  if (prefix) params.set('prefix', prefix);
  if (maxKeys) params.set('maxKeys', String(maxKeys));
  if (continuationToken) params.set('continuationToken', continuationToken);

  const url = `/api/runtime/files/${encodeURIComponent(bucket)}?${params.toString()}`;

  const result = await RuntimeREST.instance.get<ListObjectsResponse>(
    url,
    createAuthHeaders(token)
  );
  return result.data;
}

export async function uploadFile(
  token: string,
  connectionId: string,
  bucket: string,
  file: File,
  key?: string
): Promise<{ key: string; size: number }> {
  const formData = new FormData();
  formData.append('file', file);
  if (key) {
    formData.append('key', key);
  }

  const url = appendConnectionId(
    `/api/runtime/files/${encodeURIComponent(bucket)}`,
    connectionId
  );

  const result = await RuntimeREST.instance.post(url, formData, {
    headers: {
      Authorization: `Bearer ${token}`,
      'Content-Type': 'multipart/form-data',
    },
  });
  return result.data;
}

export async function downloadFile(
  token: string,
  connectionId: string,
  bucket: string,
  key: string
): Promise<Blob> {
  const url = appendConnectionId(
    `/api/runtime/files/${encodeURIComponent(bucket)}/${encodeURIComponent(key)}`,
    connectionId
  );
  const result = await RuntimeREST.instance.get(url, {
    ...createAuthHeaders(token),
    responseType: 'blob',
  });
  return result.data;
}

export async function deleteFile(
  token: string,
  connectionId: string,
  bucket: string,
  key: string
): Promise<void> {
  const url = appendConnectionId(
    `/api/runtime/files/${encodeURIComponent(bucket)}/${encodeURIComponent(key)}`,
    connectionId
  );
  await RuntimeREST.instance.delete(url, createAuthHeaders(token));
}
