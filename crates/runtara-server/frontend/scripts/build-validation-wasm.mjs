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
const wasmCrate = path.join(
  workspaceRoot,
  'crates/runtara-workflow-validation-wasm'
);
const outputDir = path.join(
  workspaceRoot,
  'crates/runtara-server/frontend/src/wasm/workflow-validation'
);
const fingerprintFile = path.join(
  outputDir,
  'runtara_workflow_validation.fingerprint'
);

const requiredOutputs = [
  'package.json',
  'runtara_workflow_validation.d.ts',
  'runtara_workflow_validation.js',
  'runtara_workflow_validation_bg.wasm',
  'runtara_workflow_validation_bg.wasm.d.ts',
];

const inputs = [
  'Cargo.toml',
  'Cargo.lock',
  'crates/runtara-workflow-validation-wasm/Cargo.toml',
  'crates/runtara-workflow-validation-wasm/src',
  'crates/runtara-workflows/Cargo.toml',
  'crates/runtara-workflows/src',
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

const fingerprint = computeFingerprint();
const outputsExist = requiredOutputs.every((name) =>
  existsSync(path.join(outputDir, name))
);
const currentFingerprint =
  existsSync(fingerprintFile) &&
  readFileSync(fingerprintFile, 'utf8').trim() === fingerprint;

if (outputsExist && currentFingerprint) {
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

mkdirSync(outputDir, { recursive: true });

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
    'runtara_workflow_validation',
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
