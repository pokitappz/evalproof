#!/usr/bin/env node
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { createRequire } from 'node:module';
import { existsSync } from 'node:fs';
const suffix = process.platform === 'win32' ? '.exe' : '';
let binary = fileURLToPath(new URL(`./bin/evalproof${suffix}`, import.meta.url));
if (!existsSync(binary)) {
  try { binary = createRequire(import.meta.url).resolve(`@evalproof/cli-${process.platform}-${process.arch}/bin/evalproof${suffix}`); }
  catch { /* Source checkout: the actionable error below explains packaging. */ }
}
const result = spawnSync(binary, process.argv.slice(2), { stdio: 'inherit' });
if (result.error) {
  process.stderr.write('This package has no matching binary. Use a platform release package or build with scripts/package.py.\n');
}
process.exitCode = result.status ?? 2;
