#!/usr/bin/env node

import { spawnSync } from 'node:child_process';
import {
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const FINGERPRINT_VERSION = 'runtara-validation-wasm-v1';

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const workspaceRoot = path.resolve(scriptDir, '../../../..');
const wasmCrate = path.join(workspaceRoot, 'crates/runtara-validation-wasm');
const outputDir = path.join(
  workspaceRoot,
  'crates/runtara-server/frontend/src/wasm/validation'
);
const fingerprintFile = path.join(outputDir, 'runtara_validation.fingerprint');

const requiredOutputs = [
  'package.json',
  'runtara_validation.d.ts',
  'runtara_validation.js',
  'runtara_validation_bg.wasm',
  'runtara_validation_bg.wasm.d.ts',
];

const generatedOutputNames = new Set([
  ...requiredOutputs,
  'runtara_validation.fingerprint',
]);

const inputs = [
  'Cargo.toml',
  'Cargo.lock',
  'crates/runtara-validation-wasm/Cargo.toml',
  'crates/runtara-validation-wasm/src',
  'crates/runtara-workflows/Cargo.toml',
  'crates/runtara-workflows/src',
  // The validator shares `reference_path` with the workflow stdlib, so a
  // stdlib edit changes what the browser WASM accepts.
  'crates/runtara-workflow-stdlib/Cargo.toml',
  'crates/runtara-workflow-stdlib/src',
  'crates/runtara-dsl/Cargo.toml',
  'crates/runtara-dsl/src',
  'crates/runtara-agents/Cargo.toml',
  'crates/runtara-agents/src',
  'crates/runtara-ai/Cargo.toml',
  'crates/runtara-ai/src',
  'crates/runtara-http/Cargo.toml',
  'crates/runtara-http/src',
].map((input) => path.join(workspaceRoot, input));

function collectFiles(inputPath, files) {
  if (!existsSync(inputPath)) {
    return;
  }

  const stat = statSync(inputPath);
  if (stat.isFile()) {
    files.push(inputPath);
    return;
  }

  if (!stat.isDirectory()) {
    return;
  }

  for (const entry of readdirSync(inputPath)) {
    collectFiles(path.join(inputPath, entry), files);
  }
}

function fnv1a64Update(hash, bytes) {
  let value = hash;
  for (const byte of bytes) {
    value ^= BigInt(byte);
    value = BigInt.asUintN(64, value * 0x100000001b3n);
  }
  return value;
}

// Must stay byte-identical to `validation_wasm_fingerprint` in
// `crates/runtara-server/build.rs`: both write the same
// `runtara_validation.fingerprint`, so any disagreement makes each side treat
// the other's value as stale and rebuild the WASM on every build. `files.sort()`
// compares whole path strings; the Rust side sorts the same way rather than with
// `PathBuf`'s component-wise `Ord`.
function computeFingerprint() {
  const files = [];
  for (const input of inputs) {
    collectFiles(input, files);
  }
  files.sort();

  let hash = 0xcbf29ce484222325n;
  hash = fnv1a64Update(hash, Buffer.from(FINGERPRINT_VERSION));

  for (const file of files) {
    const relative = path.relative(workspaceRoot, file);
    hash = fnv1a64Update(hash, Buffer.from(relative));
    hash = fnv1a64Update(hash, Buffer.from([0]));
    hash = fnv1a64Update(hash, readFileSync(file));
    hash = fnv1a64Update(hash, Buffer.from([0]));
  }

  return hash.toString(16).padStart(16, '0');
}

function cleanOutputDir({ keepCurrent }) {
  mkdirSync(outputDir, { recursive: true });

  for (const entry of readdirSync(outputDir)) {
    if (keepCurrent && generatedOutputNames.has(entry)) {
      continue;
    }

    rmSync(path.join(outputDir, entry), { recursive: true, force: true });
  }
}

const fingerprint = computeFingerprint();
const outputsExist = requiredOutputs.every((name) =>
  existsSync(path.join(outputDir, name))
);
const currentFingerprint =
  existsSync(fingerprintFile) &&
  readFileSync(fingerprintFile, 'utf8').trim() === fingerprint;

if (outputsExist && currentFingerprint) {
  cleanOutputDir({ keepCurrent: true });
  console.log('Browser validation WASM is up-to-date');
  process.exit(0);
}

const versionCheck = spawnSync('wasm-pack', ['--version'], {
  cwd: workspaceRoot,
  stdio: 'ignore',
});
if (versionCheck.error || versionCheck.status !== 0) {
  console.error(
    'wasm-pack is required to build browser validation WASM. Install it with: cargo install wasm-pack --locked'
  );
  process.exit(1);
}

cleanOutputDir({ keepCurrent: false });

const result = spawnSync(
  'wasm-pack',
  [
    'build',
    wasmCrate,
    '--target',
    'web',
    '--out-dir',
    outputDir,
    '--out-name',
    'runtara_validation',
  ],
  {
    cwd: workspaceRoot,
    stdio: 'inherit',
    env: {
      ...process.env,
      CARGO_TARGET_DIR: path.join(workspaceRoot, 'target/validation-wasm-pack'),
    },
  }
);

if (result.error) {
  throw result.error;
}
if (result.status !== 0) {
  process.exit(result.status ?? 1);
}

rmSync(path.join(outputDir, '.gitignore'), { force: true });
writeFileSync(fingerprintFile, `${fingerprint}\n`);
console.log(`Browser validation WASM generated at ${outputDir}`);
