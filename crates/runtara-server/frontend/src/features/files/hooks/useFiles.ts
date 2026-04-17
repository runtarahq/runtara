import { useQueryClient } from '@tanstack/react-query';
import { useCustomQuery, useCustomMutation } from '@/shared/hooks/api';
import { queryKeys } from '@/shared/queries/query-keys';
import {
  listBuckets,
  createBucket,
  deleteBucket,
  listObjects,
  uploadFile,
  deleteFile,
  type BucketDto,
  type ListObjectsResponse,
} from '../queries';

// ============================================================================
// Bucket Hooks
// ============================================================================

export function useBuckets(connectionId: string | undefined) {
  return useCustomQuery<BucketDto[]>({
    queryKey: queryKeys.files.buckets(connectionId),
    queryFn: (token) => listBuckets(token, connectionId!),
    enabled: !!connectionId,
  });
}

export function useCreateBucket(connectionId: string | undefined) {
  const queryClient = useQueryClient();

  return useCustomMutation<void, string>({
    mutationFn: (token, name) => createBucket(token, connectionId!, name),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: queryKeys.files.buckets(connectionId),
      });
    },
  });
}

export function useDeleteBucket(connectionId: string | undefined) {
  const queryClient = useQueryClient();

  return useCustomMutation<void, string>({
    mutationFn: (token, name) => deleteBucket(token, connectionId!, name),
    onSuccess: () => {
      queryClient.invalidateQueries({
        queryKey: queryKeys.files.buckets(connectionId),
      });
    },
  });
}

// ============================================================================
// File / Object Hooks
// ============================================================================

export function useFiles(
  connectionId: string | undefined,
  bucket: string | undefined,
  prefix?: string,
  maxKeys?: number
) {
  return useCustomQuery<ListObjectsResponse>({
    queryKey: queryKeys.files.list({ connectionId, bucket, prefix, maxKeys }),
    queryFn: (token) =>
      listObjects(token, connectionId!, bucket!, prefix, maxKeys),
    enabled: !!connectionId && !!bucket,
  });
}

export function useUploadFile(connectionId: string | undefined) {
  const queryClient = useQueryClient();

  return useCustomMutation<
    { key: string; size: number },
    { bucket: string; file: File; key?: string }
  >({
    mutationFn: (token, { bucket, file, key }) =>
      uploadFile(token, connectionId!, bucket, file, key),
    onSuccess: (_data, variables) => {
      queryClient.invalidateQueries({
        queryKey: queryKeys.files.list({
          connectionId,
          bucket: variables.bucket,
        }),
      });
    },
  });
}

export function useDeleteFile(connectionId: string | undefined) {
  const queryClient = useQueryClient();

  return useCustomMutation<void, { bucket: string; key: string }>({
    mutationFn: (token, { bucket, key }) =>
      deleteFile(token, connectionId!, bucket, key),
    onSuccess: (_data, variables) => {
      queryClient.invalidateQueries({
        queryKey: queryKeys.files.list({
          connectionId,
          bucket: variables.bucket,
        }),
      });
    },
  });
}
