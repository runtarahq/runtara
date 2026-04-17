import { useState, useRef, useEffect, useMemo } from 'react';
import { toast } from 'sonner';
import {
  FolderPlus,
  Upload,
  Trash2,
  Download,
  File as FileIcon,
  Image,
  FileText,
  FolderOpen,
  Loader2,
  HardDrive,
} from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select';
import { usePageTitle } from '@/shared/hooks/usePageTitle';
import { TilesPage } from '@/shared/components/tiles-page';
import {
  useBuckets,
  useCreateBucket,
  useDeleteBucket,
  useFiles,
  useUploadFile,
  useDeleteFile,
} from '@/features/files/hooks/useFiles';
import { downloadFile } from '@/features/files/queries';
import { useConnections } from '@/features/connections/hooks/useConnections';
import { useAuth } from 'react-oidc-context';
import type { FileObjectDto } from '@/features/files/queries';

function formatBytes(bytes: number): string {
  if (bytes === 0) return '0 B';
  const k = 1024;
  const sizes = ['B', 'KB', 'MB', 'GB'];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return `${parseFloat((bytes / Math.pow(k, i)).toFixed(1))} ${sizes[i]}`;
}

function formatDate(iso: string): string {
  if (!iso) return '';
  try {
    return new Date(iso).toLocaleDateString(undefined, {
      year: 'numeric',
      month: 'short',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
    });
  } catch {
    return iso;
  }
}

/**
 * Validate S3 bucket name.
 * Rules: 3-63 chars, lowercase letters/numbers/hyphens, no leading/trailing hyphen.
 */
function validateBucketName(name: string): string | null {
  if (name.length < 3) return 'Bucket name must be at least 3 characters';
  if (name.length > 63) return 'Bucket name must be at most 63 characters';
  if (!/^[a-z0-9][a-z0-9-]*[a-z0-9]$/.test(name) && name.length >= 3)
    return 'Only lowercase letters, numbers, and hyphens allowed (no leading/trailing hyphen)';
  if (/[^a-z0-9-]/.test(name))
    return 'Only lowercase letters, numbers, and hyphens allowed';
  return null;
}

function getFileIcon(key: string) {
  const ext = key.split('.').pop()?.toLowerCase();
  if (['png', 'jpg', 'jpeg', 'gif', 'webp', 'svg', 'bmp'].includes(ext || ''))
    return <Image size={16} className="text-purple-500" />;
  if (['txt', 'csv', 'json', 'xml', 'md', 'log'].includes(ext || ''))
    return <FileText size={16} className="text-blue-500" />;
  return <FileIcon size={16} className="text-slate-400" />;
}

export function Files() {
  usePageTitle('File Storage');
  const auth = useAuth();

  // Connection selector state
  const { data: allConnections, isLoading: connectionsLoading } =
    useConnections();
  const s3Connections = useMemo(
    () =>
      (allConnections ?? []).filter(
        (c) => c.connectionType?.category === 'file_storage'
      ),
    [allConnections]
  );
  const [selectedConnectionId, setSelectedConnectionId] = useState<
    string | undefined
  >();

  // Auto-select when connections load: prefer the default, else the only one
  useEffect(() => {
    if (selectedConnectionId || s3Connections.length === 0) return;
    const defaultConn = s3Connections.find((c) => c.isDefaultFileStorage);
    if (defaultConn) {
      setSelectedConnectionId(defaultConn.id);
    } else if (s3Connections.length === 1) {
      setSelectedConnectionId(s3Connections[0].id);
    }
  }, [s3Connections, selectedConnectionId]);

  // Reset bucket selection when connection changes
  const [selectedBucket, setSelectedBucket] = useState<string | undefined>();
  const [newBucketName, setNewBucketName] = useState('');
  const [showNewBucket, setShowNewBucket] = useState(false);
  const fileInputRef = useRef<HTMLInputElement>(null);

  const handleConnectionChange = (connId: string) => {
    setSelectedConnectionId(connId);
    setSelectedBucket(undefined);
  };

  const { data: buckets, isLoading: bucketsLoading } =
    useBuckets(selectedConnectionId);
  const { data: filesData, isLoading: filesLoading } = useFiles(
    selectedConnectionId,
    selectedBucket
  );
  const createBucketMutation = useCreateBucket(selectedConnectionId);
  const deleteBucketMutation = useDeleteBucket(selectedConnectionId);
  const uploadFileMutation = useUploadFile(selectedConnectionId);
  const deleteFileMutation = useDeleteFile(selectedConnectionId);

  const bucketNameError = newBucketName
    ? validateBucketName(newBucketName)
    : null;

  const handleCreateBucket = () => {
    const name = newBucketName.trim();
    if (!name) return;
    const error = validateBucketName(name);
    if (error) {
      toast.error(error);
      return;
    }
    createBucketMutation.mutate(name, {
      onSuccess: () => {
        setSelectedBucket(newBucketName.trim());
        setNewBucketName('');
        setShowNewBucket(false);
        toast.success('Bucket created');
      },
    });
  };

  const handleDeleteBucket = (name: string) => {
    if (!confirm(`Delete bucket "${name}"? It must be empty.`)) return;
    deleteBucketMutation.mutate(name, {
      onSuccess: () => {
        if (selectedBucket === name) setSelectedBucket(undefined);
        toast.success('Bucket deleted');
      },
    });
  };

  const handleFileUpload = (e: React.ChangeEvent<HTMLInputElement>) => {
    const files = e.target.files;
    if (!files || !selectedBucket) return;
    Array.from(files).forEach((file) => {
      uploadFileMutation.mutate(
        { bucket: selectedBucket, file },
        {
          onSuccess: () => toast.success(`Uploaded: ${file.name}`),
        }
      );
    });
    e.target.value = '';
  };

  const handleDownload = async (file: FileObjectDto) => {
    if (!selectedBucket || !selectedConnectionId) return;
    const token = auth.user?.access_token;
    if (!token) return;
    try {
      const blob = await downloadFile(
        token,
        selectedConnectionId,
        selectedBucket,
        file.key
      );
      const url = URL.createObjectURL(blob);
      const a = document.createElement('a');
      a.href = url;
      a.download = file.key.split('/').pop() || file.key;
      a.click();
      URL.revokeObjectURL(url);
    } catch {
      toast.error('Download failed');
    }
  };

  const handleDeleteFile = (file: FileObjectDto) => {
    if (!selectedBucket) return;
    if (!confirm(`Delete "${file.key}"?`)) return;
    deleteFileMutation.mutate(
      { bucket: selectedBucket, key: file.key },
      {
        onSuccess: () => toast.success('File deleted'),
      }
    );
  };

  return (
    <TilesPage
      kicker="Storage"
      title="File Storage"
      action={
        <div className="flex items-center gap-3">
          {/* S3 connection selector */}
          {!connectionsLoading && s3Connections.length > 1 && (
            <Select
              value={selectedConnectionId ?? ''}
              onValueChange={handleConnectionChange}
            >
              <SelectTrigger className="w-52">
                <HardDrive size={14} className="mr-2 shrink-0" />
                <SelectValue placeholder="Select connection" />
              </SelectTrigger>
              <SelectContent>
                {s3Connections.map((c) => (
                  <SelectItem key={c.id} value={c.id}>
                    {c.title}
                    {c.isDefaultFileStorage ? ' (default)' : ''}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          )}

          {selectedBucket && (
            <>
              <Button
                variant="outline"
                className="h-9 rounded-full"
                onClick={() => fileInputRef.current?.click()}
                disabled={uploadFileMutation.isPending}
              >
                {uploadFileMutation.isPending ? (
                  <Loader2 size={16} className="mr-2 animate-spin" />
                ) : (
                  <Upload size={16} className="mr-2" />
                )}
                Upload
              </Button>
              <input
                ref={fileInputRef}
                type="file"
                multiple
                className="hidden"
                onChange={handleFileUpload}
              />
            </>
          )}
        </div>
      }
    >
      {/* No S3 connections state */}
      {!connectionsLoading && s3Connections.length === 0 ? (
        <div className="flex flex-col items-center justify-center py-20 text-slate-400">
          <HardDrive size={48} className="mb-4" />
          <p className="text-lg font-medium">No file storage connections</p>
          <p className="text-sm">
            Create an S3-compatible connection to use the file browser.
          </p>
        </div>
      ) : !selectedConnectionId ? (
        <div className="flex flex-col items-center justify-center py-20 text-slate-400">
          <HardDrive size={48} className="mb-4" />
          <p className="text-lg font-medium">Select a connection</p>
          <p className="text-sm">
            Choose an S3 connection from the dropdown above to browse files.
          </p>
        </div>
      ) : (
        <div className="flex gap-6 min-h-[60vh]">
          {/* Bucket sidebar */}
          <div className="w-60 shrink-0">
            <div className="flex items-center justify-between mb-3">
              <h2 className="text-sm font-semibold text-slate-600 dark:text-slate-400 uppercase tracking-wider">
                Buckets
              </h2>
              <Button
                variant="ghost"
                size="sm"
                onClick={() => setShowNewBucket(true)}
              >
                <FolderPlus size={14} />
              </Button>
            </div>

            {showNewBucket && (
              <div className="mb-3">
                <div className="flex gap-1">
                  <input
                    type="text"
                    placeholder="bucket-name"
                    className={`flex-1 text-sm px-2 py-1 border rounded bg-white dark:bg-slate-900 ${
                      bucketNameError ? 'border-red-400' : ''
                    }`}
                    value={newBucketName}
                    onChange={(e) =>
                      setNewBucketName(e.target.value.toLowerCase())
                    }
                    onKeyDown={(e) => e.key === 'Enter' && handleCreateBucket()}
                    autoFocus
                  />
                  <Button
                    size="sm"
                    variant="ghost"
                    onClick={handleCreateBucket}
                    disabled={!!bucketNameError || !newBucketName}
                  >
                    OK
                  </Button>
                </div>
                {bucketNameError && (
                  <p className="text-xs text-red-500 mt-1 px-1">
                    {bucketNameError}
                  </p>
                )}
              </div>
            )}

            {bucketsLoading ? (
              <div className="flex justify-center py-8">
                <Loader2 className="animate-spin text-slate-400" />
              </div>
            ) : (
              <div className="space-y-0.5">
                {buckets?.map((b) => (
                  <div
                    key={b.name}
                    className={`group flex items-center justify-between px-3 py-2 rounded-md cursor-pointer text-sm transition-colors ${
                      selectedBucket === b.name
                        ? 'bg-blue-50 text-blue-700 dark:bg-blue-900/20 dark:text-blue-300'
                        : 'text-slate-700 hover:bg-slate-100 dark:text-slate-300 dark:hover:bg-slate-800'
                    }`}
                    onClick={() => setSelectedBucket(b.name)}
                  >
                    <div className="flex items-center gap-2 truncate">
                      <FolderOpen size={14} />
                      <span className="truncate">{b.name}</span>
                    </div>
                    <button
                      className="opacity-0 group-hover:opacity-100 transition-opacity text-slate-400 hover:text-red-500"
                      onClick={(e) => {
                        e.stopPropagation();
                        handleDeleteBucket(b.name);
                      }}
                    >
                      <Trash2 size={12} />
                    </button>
                  </div>
                ))}
                {(!buckets || buckets.length === 0) && (
                  <p className="text-sm text-slate-400 px-3 py-4">
                    No buckets yet. Create one to get started.
                  </p>
                )}
              </div>
            )}
          </div>

          {/* File list */}
          <div className="flex-1 bg-white dark:bg-slate-900 rounded-xl border border-slate-200 dark:border-slate-800">
            {!selectedBucket ? (
              <div className="flex flex-col items-center justify-center h-full py-20 text-slate-400">
                <FolderOpen size={48} className="mb-4" />
                <p className="text-lg font-medium">Select a bucket</p>
                <p className="text-sm">
                  Choose a bucket from the sidebar to browse files
                </p>
              </div>
            ) : filesLoading ? (
              <div className="flex justify-center py-20">
                <Loader2 className="animate-spin text-slate-400" size={32} />
              </div>
            ) : !filesData?.files?.length ? (
              <div className="flex flex-col items-center justify-center h-full py-20 text-slate-400">
                <Upload size={48} className="mb-4" />
                <p className="text-lg font-medium">No files</p>
                <p className="text-sm mb-4">
                  Upload files to this bucket to see them here
                </p>
                <Button
                  variant="outline"
                  onClick={() => fileInputRef.current?.click()}
                >
                  <Upload size={14} className="mr-2" />
                  Upload files
                </Button>
              </div>
            ) : (
              <div className="overflow-x-auto">
                <table className="w-full text-sm">
                  <thead>
                    <tr className="border-b border-slate-100 dark:border-slate-800">
                      <th className="text-left px-4 py-3 font-medium text-slate-500">
                        Name
                      </th>
                      <th className="text-right px-4 py-3 font-medium text-slate-500">
                        Size
                      </th>
                      <th className="text-left px-4 py-3 font-medium text-slate-500">
                        Last Modified
                      </th>
                      <th className="text-right px-4 py-3 font-medium text-slate-500 w-24">
                        Actions
                      </th>
                    </tr>
                  </thead>
                  <tbody>
                    {filesData.files.map((file) => (
                      <tr
                        key={file.key}
                        className="border-b border-slate-50 dark:border-slate-800/50 hover:bg-slate-50 dark:hover:bg-slate-800/30 transition-colors"
                      >
                        <td className="px-4 py-3">
                          <div className="flex items-center gap-2">
                            {getFileIcon(file.key)}
                            <span
                              className="truncate max-w-md"
                              title={file.key}
                            >
                              {file.key}
                            </span>
                          </div>
                        </td>
                        <td className="px-4 py-3 text-right text-slate-500 whitespace-nowrap">
                          {formatBytes(file.size)}
                        </td>
                        <td className="px-4 py-3 text-slate-500 whitespace-nowrap">
                          {formatDate(file.lastModified)}
                        </td>
                        <td className="px-4 py-3 text-right">
                          <div className="flex items-center justify-end gap-1">
                            <button
                              className="p-1.5 rounded hover:bg-slate-100 dark:hover:bg-slate-800 text-slate-400 hover:text-blue-600 transition-colors"
                              onClick={() => handleDownload(file)}
                              title="Download"
                            >
                              <Download size={14} />
                            </button>
                            <button
                              className="p-1.5 rounded hover:bg-slate-100 dark:hover:bg-slate-800 text-slate-400 hover:text-red-600 transition-colors"
                              onClick={() => handleDeleteFile(file)}
                              title="Delete"
                            >
                              <Trash2 size={14} />
                            </button>
                          </div>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
                {filesData.nextContinuationToken && (
                  <div className="px-4 py-3 text-center text-sm text-slate-400">
                    More files available — pagination coming soon
                  </div>
                )}
              </div>
            )}
          </div>
        </div>
      )}
    </TilesPage>
  );
}
