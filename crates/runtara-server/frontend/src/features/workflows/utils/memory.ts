/**
 * A workflow's memory is a number of MiB; S/M/L/XL are presets that fill
 * it in. A run's linear memory is capped at this value.
 */
export const MEMORY_PRESET_MIB: Record<string, number> = {
  S: 8,
  M: 64,
  L: 128,
  XL: 256,
};

/** Memory a workflow gets when none is set, in MiB (server default). */
export const DEFAULT_MEMORY_MB = 256;

/** The largest memory a workflow may ask for, in MiB (server limit). */
export const MAX_MEMORY_MB = 65_536;
