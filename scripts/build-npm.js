const fs = require('fs');
const path = require('path');
const execSync = require('child_process').execSync;

const version = '0.8.1';
const localMode = process.argv.includes('--local');
const skipBuild = process.argv.includes('--skip-build');

const platformPackages = [
  { name: 'amae-darwin-arm64', os: 'darwin', cpu: 'arm64', binary: 'amae' },
  { name: 'amae-darwin-x64', os: 'darwin', cpu: 'x64', binary: 'amae' },
  { name: 'amae-linux-x64', os: 'linux', cpu: 'x64', binary: 'amae' },
  { name: 'amae-win32-x64', os: 'win32', cpu: 'x64', binary: 'amae.exe' }
];

function buildRust() {
  if (skipBuild) {
    console.log('Skipping Rust build (--skip-build flag set).');
    return;
  }
  console.log('Building rust release binary...');
  execSync('cargo build --release', { stdio: 'inherit' });
}

function ensureDir(dir) {
  if (!fs.existsSync(dir)) {
    fs.mkdirSync(dir, { recursive: true });
  }
}

function generatePlatformPackages() {
  const currentPlatform = process.platform;
  const currentArch = process.arch;

  for (const pkg of platformPackages) {
    const pkgDir = path.join(__dirname, '..', 'npm', pkg.name);
    const binDir = path.join(pkgDir, 'bin');
    ensureDir(binDir);

    const pkgJson = {
      name: pkg.name,
      version: version,
      description: `Platform-specific binary for ${pkg.name}`,
      os: [pkg.os],
      cpu: [pkg.cpu],
      bin: {
        [pkg.name]: `bin/${pkg.binary}`
      }
    };

    fs.writeFileSync(
      path.join(pkgDir, 'package.json'),
      JSON.stringify(pkgJson, null, 2)
    );

    if (pkg.os === currentPlatform && pkg.cpu === currentArch) {
      const srcBinary = currentPlatform === 'win32'
        ? path.join(__dirname, '..', 'target', 'release', 'amae.exe')
        : path.join(__dirname, '..', 'target', 'release', 'amae');

      if (fs.existsSync(srcBinary)) {
        const destBinary = path.join(binDir, pkg.binary);
        fs.copyFileSync(srcBinary, destBinary);
        fs.chmodSync(destBinary, 0o755);
        console.log(`Copied binary to ${pkg.name}`);
      } else {
        console.error(`Source binary not found at ${srcBinary}`);
      }
    }
  }
}

function generateMainPackage() {
  const mainDir = path.join(__dirname, '..', 'npm', 'amae-cli');
  const binDir = path.join(mainDir, 'bin');
  ensureDir(binDir);

  const optionalDependencies = {};
  for (const pkg of platformPackages) {
    optionalDependencies[pkg.name] = localMode ? `file:../${pkg.name}` : version;
  }

  const pkgJson = {
    name: 'amae-cli',
    version: version,
    description: 'Ultra-fast package manager for JS/TS written in Rust',
    bin: {
      amae: 'bin/amae'
    },
    keywords: ['package-manager', 'npm', 'rust', 'fast'],
    license: 'MIT',
    optionalDependencies: optionalDependencies
  };

  fs.writeFileSync(
    path.join(mainDir, 'package.json'),
    JSON.stringify(pkgJson, null, 2)
  );

  const wrapperCode = `#!/usr/bin/env node

const fs = require('fs');
const path = require('path');
const spawn = require('child_process').spawnSync;

const platform = process.platform;
const arch = process.arch;

const packageMap = {
  'darwin-arm64': 'amae-darwin-arm64',
  'darwin-x64': 'amae-darwin-x64',
  'linux-x64': 'amae-linux-x64',
  'win32-x64': 'amae-win32-x64'
};

const key = \`\${platform}-\${arch}\`;
const pkgName = packageMap[key];

if (!pkgName) {
  console.error(\`Unsupported platform/architecture: \${key}\`);
  process.exit(1);
}

let binName = platform === 'win32' ? 'amae.exe' : 'amae';
let binPath = '';

try {
  binPath = require.resolve(\`\${pkgName}/bin/\${binName}\`);
} catch (e) {
  const localPaths = [
    path.join(__dirname, '..', 'node_modules', pkgName, 'bin', binName),
    path.join(__dirname, '..', '..', pkgName, 'bin', binName)
  ];
  for (const p of localPaths) {
    if (fs.existsSync(p)) {
      binPath = p;
      break;
    }
  }
}

if (!binPath || !fs.existsSync(binPath)) {
  console.error(\`Binary not found for \${pkgName}. Try reinstalling.\`);
  process.exit(1);
}

const result = spawn(binPath, process.argv.slice(2), {
  stdio: 'inherit',
  windowsHide: true
});

if (result.error) {
  console.error('Failed to run binary:', result.error);
  process.exit(1);
}

process.exit(result.status ?? 0);
`;

  const wrapperFile = path.join(binDir, 'amae');
  fs.writeFileSync(wrapperFile, wrapperCode);
  fs.chmodSync(wrapperFile, 0o755);
  console.log('Generated main package and JS wrapper');
}

function main() {
  buildRust();
  generatePlatformPackages();
  generateMainPackage();
  console.log('Build completed successfully.');
}

main();
