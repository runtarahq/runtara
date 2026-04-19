/**
 * File data types for handling file inputs/outputs in workflows and operators.
 *
 * The runtime supports a `file` type that accepts two formats:
 * 1. FileData object (full format) with content, filename, and mimeType
 * 2. Plain base64 string (shorthand)
 */

/**
 * Represents file data with base64-encoded content.
 * This is the full format for file type values.
 */
export interface FileData {
  /** Base64-encoded file content (required) */
  content: string;
  /** Original filename (optional) */
  filename?: string;
  /** MIME type of the file (optional) */
  mimeType?: string;
}

/**
 * File field value can be either a FileData object or a plain base64 string.
 */
export type FileFieldValue = FileData | string;

/**
 * Maximum file size in bytes (50 MB)
 */
export const MAX_FILE_SIZE_BYTES = 50 * 1024 * 1024;

/**
 * Maximum file size in human-readable format
 */
export const MAX_FILE_SIZE_DISPLAY = '50 MB';
