#!/usr/bin/env node
'use strict';

const fs = require('fs');
const path = require('path');
const { spawn } = require('child_process');

const packageRoot = path.resolve(__dirname, '..');
const repoRoot = path.resolve(packageRoot, '..', '..');

function platformKey() {
  const platform = process.platform;
  const arch = process.arch;
  const supported = new Set([
    'darwin-arm64',
    'darwin-x64',
    'linux-arm64',
    'linux-x64',
    'win32-arm64',
    'win32-x64',
  ]);
  const key = `${platform}-${arch}`;
  if (!supported.has(key)) {
    throw new Error(`Unsupported platform: ${key}`);
  }
  return key;
}

function executableName() {
  return process.platform === 'win32' ? 'codebase-mcp.exe' : 'codebase-mcp';
}

function candidates() {
  const name = executableName();
  const items = [];
  if (process.env.CODEBASE_MCP_BINARY) {
    items.push(process.env.CODEBASE_MCP_BINARY);
  }
  items.push(path.join(packageRoot, 'native', platformKey(), name));
  items.push(path.join(repoRoot, 'target', 'release', name));
  return items;
}

function findBinary() {
  for (const candidate of candidates()) {
    if (candidate && fs.existsSync(candidate)) {
      return candidate;
    }
  }
  return null;
}

function main() {
  let binary;
  try {
    binary = findBinary();
  } catch (error) {
    console.error(error.message);
    process.exit(1);
  }

  if (!binary) {
    console.error([
      'codebase-mcp binary was not found for this platform.',
      `Expected bundled path: native/${platformKey()}/${executableName()}`,
      'Install from GitHub Releases, run cargo build --release, or set CODEBASE_MCP_BINARY.',
    ].join('\n'));
    process.exit(1);
  }

  const child = spawn(binary, process.argv.slice(2), {
    stdio: 'inherit',
    windowsHide: true,
  });

  child.on('error', (error) => {
    console.error(error.message);
    process.exit(1);
  });

  child.on('exit', (code, signal) => {
    if (signal) {
      process.kill(process.pid, signal);
      return;
    }
    process.exit(code ?? 0);
  });
}

main();
