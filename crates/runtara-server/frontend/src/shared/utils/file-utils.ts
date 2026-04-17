/**
 * File handling utilities for converting files to base64 and FileData format.
 */

import {
  FileData,
  FileFieldValue,
  MAX_FILE_SIZE_BYTES,
  MAX_FILE_SIZE_DISPLAY,
} from '@/shared/types/file';

/**
 * Convert a File object to base64 string.
 * @param file - The File object to convert
 * @returns Promise resolving to base64 string (without data URL prefix)
 */
export async function fileToBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      const result = reader.result as string;
      // Remove data URL prefix (e.g., "data:image/png;base64,")
      const base64 = result.split(',')[1] || result;
      resolve(base64);
    };
    reader.onerror = () => reject(new Error('Failed to read file'));
    reader.readAsDataURL(file);
  });
}

/**
 * Convert a File object to FileData object.
 * @param file - The File object to convert
 * @returns Promise resolving to FileData object
 */
export async function fileToFileData(file: File): Promise<FileData> {
  const content = await fileToBase64(file);
  return {
    content,
    filename: file.name,
    mimeType: file.type || undefined,
  };
}

/**
 * Type guard to check if a value is a FileData object.
 * @param value - Value to check
 * @returns True if value is a FileData object
 */
export function isFileData(value: unknown): value is FileData {
  return (
    typeof value === 'object' &&
    value !== null &&
    'content' in value &&
    typeof (value as FileData).content === 'string'
  );
}

/**
 * Extract filename from FileFieldValue.
 * @param value - FileData object or base64 string
 * @param defaultName - Default name if not available
 * @returns Filename string
 */
export function getFilename(
  value: FileFieldValue | undefined | null,
  defaultName = 'file'
): string {
  if (!value) return defaultName;
  if (isFileData(value) && value.filename) {
    return value.filename;
  }
  return defaultName;
}

/**
 * Extract MIME type from FileFieldValue.
 * @param value - FileData object or base64 string
 * @returns MIME type string or undefined
 */
export function getMimeType(
  value: FileFieldValue | undefined | null
): string | undefined {
  if (!value) return undefined;
  if (isFileData(value)) {
    return value.mimeType;
  }
  return undefined;
}

/**
 * Get base64 content from FileFieldValue.
 * @param value - FileData object or base64 string
 * @returns Base64 content string
 */
export function getFileContent(
  value: FileFieldValue | undefined | null
): string {
  if (!value) return '';
  if (isFileData(value)) {
    return value.content;
  }
  return value;
}

/**
 * Format file size in human-readable format.
 * @param bytes - File size in bytes
 * @returns Human-readable size string
 */
export function formatFileSize(bytes: number): string {
  if (bytes === 0) return '0 Bytes';
  const k = 1024;
  const sizes = ['Bytes', 'KB', 'MB', 'GB'];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i];
}

/**
 * Validate file size against maximum allowed.
 * @param file - File to validate
 * @returns Object with valid flag and optional error message
 */
export function validateFileSize(file: File): {
  valid: boolean;
  error?: string;
} {
  if (file.size > MAX_FILE_SIZE_BYTES) {
    return {
      valid: false,
      error: `File size (${formatFileSize(file.size)}) exceeds maximum allowed (${MAX_FILE_SIZE_DISPLAY})`,
    };
  }
  return { valid: true };
}

/**
 * Parse a JSON string to FileData if possible.
 * @param value - JSON string or plain string
 * @returns Parsed FileData or null if not valid FileData JSON
 */
export function parseFileDataFromString(
  value: string | undefined | null
): FileData | null {
  if (!value) return null;
  try {
    const parsed = JSON.parse(value);
    if (isFileData(parsed)) {
      return parsed;
    }
  } catch {
    // Not valid JSON
  }
  return null;
}

/**
 * Create a data URL from FileData for download/display.
 * @param fileData - FileData object
 * @returns Data URL string
 */
export function createDataUrl(fileData: FileData): string {
  const mimeType = fileData.mimeType || 'application/octet-stream';
  return `data:${mimeType};base64,${fileData.content}`;
}

/**
 * Trigger a file download from FileData.
 * @param fileData - FileData object to download
 * @param defaultFilename - Default filename if not specified in FileData
 * @lintignore Public helper kept alongside FileData for consumer download flows.
 */
export function downloadFileData(
  fileData: FileData,
  defaultFilename = 'download'
): void {
  const dataUrl = createDataUrl(fileData);
  const filename = fileData.filename || defaultFilename;

  const link = document.createElement('a');
  link.href = dataUrl;
  link.download = filename;
  document.body.appendChild(link);
  link.click();
  document.body.removeChild(link);
}

/**
 * Normalize FileFieldValue to FileData object.
 * @param value - FileData object or base64 string
 * @returns FileData object
 */
export function normalizeToFileData(value: FileFieldValue): FileData {
  if (isFileData(value)) {
    return value;
  }
  return { content: value };
}
