#!/usr/bin/env node
/**
 * Post-install verification - downloads and tests binary on install
 * Non-blocking: doesn't fail install if GitHub Release doesn't exist yet
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
  console.log(`⏭  Skipping postinstall: unsupported platform ${key}`);
  process.exit(0);
}

const cacheDir = resolve(homedir(), '.cache', 'voidrecon', VERSION);
const binaryName = platform() === 'win32' ? 'voidrecon.exe' : 'voidrecon';
const binaryPath = resolve(cacheDir, binaryName);
const archivePath = resolve(cacheDir, asset);

async function downloadFile(url, dest) {
  const response = await fetch(url);
  if (!response.ok) return false;
  const buffer = await response.arrayBuffer();
  const { writeFileSync } = await import('fs');
  writeFileSync(dest, Buffer.from(buffer));
  return true;
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

async function verifyBinary() {
  if (existsSync(binaryPath)) {
    const result = spawnSync(binaryPath, ['--version'], { stdio: 'pipe', encoding: 'utf8' });
    if (result.status === 0) {
      console.log(`✅ voidrecon v${VERSION} already verified`);
      return true;
    }
  }

  mkdirSync(cacheDir, { recursive: true });

  const url = `${BASE_URL}/${asset}`;
  const downloaded = await downloadFile(url, archivePath);

  if (!downloaded) {
    console.log(`⏭  Postinstall: release v${VERSION} not published yet, skipping verification`);
    try { rmSync(cacheDir, { recursive: true, force: true }); } catch {}
    return true;
  }

  try {
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
      throw new Error(`Binary not found: ${binaryName}`);
    }

    if (platform() !== 'win32') {
      chmodSync(found, 0o755);
    }

    const result = spawnSync(found, ['--version'], { stdio: 'pipe', encoding: 'utf8' });
    if (result.status !== 0) {
      throw new Error(`Binary test failed: ${result.stderr || result.stdout}`);
    }

    console.log(`✅ voidrecon v${VERSION} verified: ${result.stdout.trim()}`);
    try { rmSync(archivePath); } catch {}
    return true;
  } catch (err) {
    console.warn(`⚠️  Postinstall verification failed: ${err.message}`);
    console.warn('   Binary will be downloaded on first run');
    try { rmSync(cacheDir, { recursive: true, force: true }); } catch {}
    return true;
  }
}

verifyBinary().then(() => process.exit(0)).catch(() => process.exit(0));