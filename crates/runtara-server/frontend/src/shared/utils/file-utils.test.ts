import { describe, it, expect } from 'vitest';
import {
  isFileData,
  getFilename,
  getMimeType,
  getFileContent,
  formatFileSize,
  validateFileSize,
  parseFileDataFromString,
  createDataUrl,
  normalizeToFileData,
} from './file-utils';
import { FileData } from '@/shared/types/file';

describe('file-utils', () => {
  describe('isFileData', () => {
    it('returns true for valid FileData object', () => {
      const fileData: FileData = { content: 'base64content' };
      expect(isFileData(fileData)).toBe(true);
    });

    it('returns true for FileData with all fields', () => {
      const fileData: FileData = {
        content: 'base64content',
        filename: 'test.txt',
        mimeType: 'text/plain',
      };
      expect(isFileData(fileData)).toBe(true);
    });

    it('returns false for plain string', () => {
      expect(isFileData('base64string')).toBe(false);
    });

    it('returns false for null', () => {
      expect(isFileData(null)).toBe(false);
    });

    it('returns false for undefined', () => {
      expect(isFileData(undefined)).toBe(false);
    });

    it('returns false for object without content', () => {
      expect(isFileData({ filename: 'test.txt' })).toBe(false);
    });

    it('returns false for object with non-string content', () => {
      expect(isFileData({ content: 123 })).toBe(false);
    });
  });

  describe('getFilename', () => {
    it('returns filename from FileData', () => {
      const fileData: FileData = {
        content: 'base64',
        filename: 'document.pdf',
      };
      expect(getFilename(fileData)).toBe('document.pdf');
    });

    it('returns default name for FileData without filename', () => {
      const fileData: FileData = { content: 'base64' };
      expect(getFilename(fileData)).toBe('file');
    });

    it('returns custom default name when provided', () => {
      const fileData: FileData = { content: 'base64' };
      expect(getFilename(fileData, 'download')).toBe('download');
    });

    it('returns default name for plain string', () => {
      expect(getFilename('base64string')).toBe('file');
    });

    it('returns default name for null', () => {
      expect(getFilename(null)).toBe('file');
    });

    it('returns default name for undefined', () => {
      expect(getFilename(undefined)).toBe('file');
    });
  });

  describe('getMimeType', () => {
    it('returns mimeType from FileData', () => {
      const fileData: FileData = {
        content: 'base64',
        mimeType: 'image/png',
      };
      expect(getMimeType(fileData)).toBe('image/png');
    });

    it('returns undefined for FileData without mimeType', () => {
      const fileData: FileData = { content: 'base64' };
      expect(getMimeType(fileData)).toBeUndefined();
    });

    it('returns undefined for plain string', () => {
      expect(getMimeType('base64string')).toBeUndefined();
    });

    it('returns undefined for null', () => {
      expect(getMimeType(null)).toBeUndefined();
    });

    it('returns undefined for undefined', () => {
      expect(getMimeType(undefined)).toBeUndefined();
    });
  });

  describe('getFileContent', () => {
    it('returns content from FileData', () => {
      const fileData: FileData = { content: 'base64content' };
      expect(getFileContent(fileData)).toBe('base64content');
    });

    it('returns the string directly for plain string', () => {
      expect(getFileContent('base64string')).toBe('base64string');
    });

    it('returns empty string for null', () => {
      expect(getFileContent(null)).toBe('');
    });

    it('returns empty string for undefined', () => {
      expect(getFileContent(undefined)).toBe('');
    });
  });

  describe('formatFileSize', () => {
    it('formats 0 bytes', () => {
      expect(formatFileSize(0)).toBe('0 Bytes');
    });

    it('formats bytes', () => {
      expect(formatFileSize(500)).toBe('500 Bytes');
    });

    it('formats kilobytes', () => {
      expect(formatFileSize(1024)).toBe('1 KB');
      expect(formatFileSize(1536)).toBe('1.5 KB');
    });

    it('formats megabytes', () => {
      expect(formatFileSize(1048576)).toBe('1 MB');
      expect(formatFileSize(5242880)).toBe('5 MB');
    });

    it('formats gigabytes', () => {
      expect(formatFileSize(1073741824)).toBe('1 GB');
    });
  });

  describe('validateFileSize', () => {
    it('returns valid for file under limit', () => {
      const file = new File(['content'], 'test.txt', { type: 'text/plain' });
      const result = validateFileSize(file);
      expect(result.valid).toBe(true);
      expect(result.error).toBeUndefined();
    });

    it('returns invalid for file over 50MB limit', () => {
      // Create a mock file with size over 50MB
      const largeContent = new ArrayBuffer(51 * 1024 * 1024);
      const file = new File([largeContent], 'large.bin');
      const result = validateFileSize(file);
      expect(result.valid).toBe(false);
      expect(result.error).toContain('exceeds maximum');
    });
  });

  describe('parseFileDataFromString', () => {
    it('parses valid FileData JSON string', () => {
      const json = JSON.stringify({ content: 'base64', filename: 'test.txt' });
      const result = parseFileDataFromString(json);
      expect(result).toEqual({ content: 'base64', filename: 'test.txt' });
    });

    it('returns null for invalid JSON', () => {
      expect(parseFileDataFromString('not json')).toBeNull();
    });

    it('returns null for JSON without content field', () => {
      const json = JSON.stringify({ filename: 'test.txt' });
      expect(parseFileDataFromString(json)).toBeNull();
    });

    it('returns null for null input', () => {
      expect(parseFileDataFromString(null)).toBeNull();
    });

    it('returns null for undefined input', () => {
      expect(parseFileDataFromString(undefined)).toBeNull();
    });

    it('returns null for empty string', () => {
      expect(parseFileDataFromString('')).toBeNull();
    });
  });

  describe('createDataUrl', () => {
    it('creates data URL with mimeType', () => {
      const fileData: FileData = {
        content: 'SGVsbG8gV29ybGQ=',
        mimeType: 'text/plain',
      };
      expect(createDataUrl(fileData)).toBe(
        'data:text/plain;base64,SGVsbG8gV29ybGQ='
      );
    });

    it('uses octet-stream for missing mimeType', () => {
      const fileData: FileData = { content: 'SGVsbG8=' };
      expect(createDataUrl(fileData)).toBe(
        'data:application/octet-stream;base64,SGVsbG8='
      );
    });
  });

  describe('normalizeToFileData', () => {
    it('returns FileData as-is', () => {
      const fileData: FileData = {
        content: 'base64',
        filename: 'test.txt',
      };
      expect(normalizeToFileData(fileData)).toBe(fileData);
    });

    it('converts string to FileData', () => {
      const result = normalizeToFileData('base64string');
      expect(result).toEqual({ content: 'base64string' });
    });
  });
});
