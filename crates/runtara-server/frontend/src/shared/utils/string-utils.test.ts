import { describe, expect, it } from 'vitest';
import { slugify } from './string-utils.ts';

describe('String Utilities', () => {
  describe('slugify', () => {
    it('should convert spaces to hyphens', () => {
      expect(slugify('hello world')).toBe('hello-world');
    });

    it('should convert to lowercase', () => {
      expect(slugify('Hello World')).toBe('hello-world');
    });

    it('should remove special characters', () => {
      expect(slugify('hello@world!')).toBe('helloworld');
    });

    it('should handle multiple spaces and special characters', () => {
      expect(slugify('  Hello  World!  ')).toBe('hello-world');
    });

    it('should replace underscores and multiple hyphens with a single hyphen', () => {
      expect(slugify('hello__world--test')).toBe('hello-world-test');
    });

    it('should remove leading and trailing hyphens', () => {
      expect(slugify('-hello-world-')).toBe('hello-world');
    });

    it('should return empty string when given empty string', () => {
      expect(slugify('')).toBe('');
    });
  });
});
