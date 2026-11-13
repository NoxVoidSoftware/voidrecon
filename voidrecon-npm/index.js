#!/usr/bin/env node
/**
 * voidrecon npm wrapper - downloads and executes native binary from GitHub Releases
 * Usage: npx voidrecon@latest [args...]
 */

import { spawnSync } from 'child_process';
import { platform, arch, homedir } from 'os';
import { resolve } from 'path';
import { existsSync, mkdirSync, chmodSync, rmSync } from 'fs';
import { fileURLToPath } from 'url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = resolve(__filename, '..');

const VERSION = '0.1.1';
const REPO = 'NoxVoidSoftware/voidrecon';
const BASE_URL = `https://github.com/${REPO}/releases/download/v${VERSION}`;

const TARGETS = {
  'linux-x64': 'voidrecon-x86_64-unknown-linux-gnu.tar.gz',
  'linux-arm64': 'voidrecon-aarch64-unknown-linux-gnu.tar.gz',
  'darwin-x64': 'voidrecon-x86_64-apple-darwin.tar.gz',
  'darwin-arm64': 'voidrecon-aarch64-apple-darwin.tar.gz',
  'win32-x64': 'voidrecon-x86_64-pc-windows-msvc.zip',
};

const key = `${platform()}-${arch()}`;
const asset = TARGETS[key];

if (!asset) {
  console.error(`❌ Unsupported platform: ${key}`);
  console.error('Supported: linux-x64, linux-arm64, darwin-x64, darwin-arm64, win32-x64');
  process.exit(1);
}

const cacheDir = resolve(homedir(), '.cache', 'voidrecon', VERSION);
const binaryName = platform() === 'win32' ? 'voidrecon.exe' : 'voidrecon';
const binaryPath = resolve(cacheDir, binaryName);
const archivePath = resolve(cacheDir, asset);

async function downloadFile(url, dest) {
  const response = await fetch(url);
  if (!response.ok) {
    throw new Error(`Download failed: ${response.status} ${response.statusText}`);
  }
  const buffer = await response.arrayBuffer();
  const { writeFileSync } = await import('fs');
  writeFileSync(dest, Buffer.from(buffer));
}

async function extractArchive(archive, destDir) {
  if (archive.endsWith('.tar.gz')) {
    const tar = await import('tar');
    await tar.extract({ file: archive, cwd: destDir });
  } else if (archive.endsWith('.zip')) {
    const unzipper = await import('unzipper');
    const fs = await import('fs');
    await fs.createReadStream(archive)
      .pipe(unzipper.Extract({ path: destDir }))
      .promise();
  }
}

async function ensureBinary() {
  if (existsSync(binaryPath)) {
    return binaryPath;
  }

  mkdirSync(cacheDir, { recursive: true });

  const url = `${BASE_URL}/${asset}`;
  console.error(`⬇️  Downloading voidrecon v${VERSION} for ${key}...`);

  try {
    await downloadFile(url, archivePath);
    console.error(`📦 Extracting...`);
    await extractArchive(archivePath, cacheDir);

    let found = binaryPath;
    if (!existsSync(found)) {
      const { readdirSync } = await import('fs');
      const entries = readdirSync(cacheDir, { withFileTypes: true });
      for (const entry of entries) {
        if (entry.isFile() && entry.name === binaryName) {
          found = resolve(cacheDir, entry.name);
          break;
        }
        if (entry.isDirectory()) {
          const subPath = resolve(cacheDir, entry.name, binaryName);
          if (existsSync(subPath)) {
            found = subPath;
            break;
          }
        }
      }
    }

    if (!existsSync(found)) {
      throw new Error(`Binary not found after extraction: ${binaryName}`);
    }

    if (platform() !== 'win32') {
      chmodSync(found, 0o755);
    }

    try { rmSync(archivePath); } catch {}

    console.error(`✅ Ready: ${found}`);
    return found;
  } catch (err) {
    console.error(`❌ Failed to prepare binary: ${err.message}`);
    try { rmSync(cacheDir, { recursive: true, force: true }); } catch {}
    process.exit(1);
  }
}

async function main() {
  const binary = await ensureBinary();
  const args = process.argv.slice(2);

  const result = spawnSync(binary, args, {
    stdio: 'inherit',
    env: process.env,
  });

  process.exit(result.status ?? 1);
}

main().catch(err => {
  console.error(`❌ Fatal error: ${err.message}`);
  process.exit(1);
});