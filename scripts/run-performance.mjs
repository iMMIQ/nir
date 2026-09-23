import os from 'node:os';
import fs from 'node:fs/promises';
import path from 'node:path';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const argv = process.argv.slice(2);
const modes = new Set(['ci', 'hardware', 'smoke']);
const requestedMode = modes.has(argv[0]) ? argv.shift() : undefined;
const mode = requestedMode || process.env.NIR_PERF_MODE || 'ci';

if (!modes.has(mode)) {
  fail(`Unknown performance mode "${mode}". Use ci, hardware, or smoke.`);
}

process.env.NIR_PERF_MODE = mode === 'smoke' ? 'ci' : mode;

if (mode === 'smoke') {
  process.env.NIR_PERF_MODE = 'ci';
  process.env.NIR_PERF_SMOKE = '1';
  process.env.NIR_PERF_SAMPLES ??= '2';
  process.env.NIR_PERF_CYCLES ??= '2';
  process.env.NIR_PERF_SCALE_REPETITIONS ??= '1';
} else {
  process.env.NIR_PERF_SCALE_REPETITIONS ??= '5';
}

const testFiles = mode === 'smoke' ? ['tests/performance/modules.spec.js'] : [];

if (mode === 'hardware') {
  const chromium = await resolveChromium();
  process.env.CHROMIUM = chromium;
  const chromeArgs = process.env.NIR_CHROME_ARGS?.trim();
  if (chromeArgs && hasSoftwareRenderingFlag(chromeArgs)) {
    fail('NIR_CHROME_ARGS contains a software-rendering flag; hardware mode refuses SwiftShader or other software renderers.');
  }
  if (!chromeArgs) {
    process.env.NIR_CHROME_ARGS = [
      '--ozone-platform=x11',
      '--enable-unsafe-webgpu',
      '--enable-gpu',
      '--ignore-gpu-blocklist',
      '--use-angle=vulkan',
      '--enable-features=Vulkan',
      '--use-vulkan=native',
    ].join(' ');
  }
  await writeHardwareEnvironment(chromium);
}

const playwrightCli = path.join(root, 'node_modules', '@playwright', 'test', 'cli.js');
try {
  await fs.access(playwrightCli);
} catch {
  fail('Playwright is not installed. Run `bun install --frozen-lockfile` first.');
}

const command = process.execPath;
const commandArgs = [playwrightCli, 'test', '--config', 'playwright.performance.config.js', ...testFiles, ...argv];
let executable = command;
let executableArgs = commandArgs;

if (!process.env.DISPLAY) {
  const xvfbRun = await resolveOptionalExecutable(process.env.XVFB_RUN || 'xvfb-run');
  if (!xvfbRun) {
    fail('DISPLAY is unset and xvfb-run was not found. Start a display or install/provide xvfb-run with XVFB_RUN.');
  }
  executable = xvfbRun;
  executableArgs = ['-a', '-s', '-screen 0 1920x1080x24', '--', command, ...commandArgs];
}

const result = await run(executable, executableArgs, {
  cwd: root,
  env: process.env,
  stdio: 'inherit',
});
process.exitCode = result;

async function resolveChromium() {
  const requested = process.env.CHROMIUM?.trim();
  if (requested) {
    const candidate = requested.includes(path.sep) ? path.resolve(root, requested) : await resolveOptionalExecutable(requested);
    if (candidate && await isExecutable(candidate)) return candidate;
    fail(`CHROMIUM does not name an executable Chromium binary: ${requested}`);
  }

  for (const name of ['chromium', 'chromium-browser', 'google-chrome', 'google-chrome-stable']) {
    const candidate = await resolveOptionalExecutable(name);
    if (candidate) return candidate;
  }
  fail('Hardware mode needs CHROMIUM or a system Chromium binary on PATH (including the Nix development environment).');
}

async function resolveOptionalExecutable(commandName) {
  if (commandName.includes(path.sep)) {
    const candidate = path.resolve(root, commandName);
    return await isExecutable(candidate) ? candidate : undefined;
  }
  for (const directory of (process.env.PATH || '').split(path.delimiter)) {
    if (!directory) continue;
    const candidate = path.join(directory, commandName);
    if (await isExecutable(candidate)) return candidate;
  }
  return undefined;
}

async function isExecutable(file) {
  try {
    await fs.access(file, fs.constants.X_OK);
    return true;
  } catch {
    return false;
  }
}

function hasSoftwareRenderingFlag(args) {
  return /swift\s*shader|swiftshader|llvmpipe|lavapipe|software[-_ ]?renderer|--disable-gpu|--disable-webgpu/i.test(args);
}

async function writeHardwareEnvironment(chromium) {
  const reports = path.join(root, 'reports');
  await fs.mkdir(reports, { recursive: true });
  const chromiumVersion = await commandOutput(chromium, ['--version']);
  const nvidia = await commandOutput('nvidia-smi', [
    '--query-gpu=name,driver_version,pci.bus_id',
    '--format=csv,noheader',
  ]);
  const report = {
    format: 1,
    mode: 'hardware',
    capturedAt: new Date().toISOString(),
    os: {
      platform: os.platform(),
      release: os.release(),
      version: os.version(),
      architecture: os.arch(),
    },
    cpu: {
      model: os.cpus()[0]?.model || null,
      logicalCores: os.cpus().length,
      loadAverage: os.loadavg(),
    },
    memory: {
      totalBytes: os.totalmem(),
      freeBytesAtCapture: os.freemem(),
    },
    chromium: {
      path: chromium,
      version: chromiumVersion.stdout.trim() || null,
      args: process.env.NIR_CHROME_ARGS,
    },
    gpuDriver: nvidia.code === 0
      ? { source: 'nvidia-smi', available: true, devices: nvidia.stdout.trim().split(/\r?\n/).filter(Boolean) }
      : { source: 'nvidia-smi', available: false, reason: nvidia.stderr.trim() || 'command unavailable' },
    display: process.env.DISPLAY || 'xvfb-run will provide a display',
    measurement: { gpuTime: 'unmeasured', physicalMemory: 'not measured by the engine' },
  };
  await fs.writeFile(path.join(reports, 'performance-environment.json'), `${JSON.stringify(report, null, 2)}\n`);
}

async function commandOutput(file, args) {
  return new Promise(resolve => {
    const child = spawn(file, args, { cwd: root, env: process.env, stdio: ['ignore', 'pipe', 'pipe'] });
    let stdout = '';
    let stderr = '';
    child.stdout.setEncoding('utf8').on('data', chunk => { stdout += chunk; });
    child.stderr.setEncoding('utf8').on('data', chunk => { stderr += chunk; });
    child.once('error', error => resolve({ code: -1, stdout, stderr: `${stderr}${error.message}` }));
    child.once('close', code => resolve({ code: code ?? -1, stdout, stderr }));
  });
}

function run(file, args, options) {
  return new Promise(resolve => {
    const child = spawn(file, args, options);
    child.once('error', error => {
      console.error(`Could not start ${file}: ${error.message}`);
      resolve(1);
    });
    child.once('close', (code, signal) => {
      if (signal) console.error(`Performance runner stopped by ${signal}.`);
      resolve(code ?? 1);
    });
  });
}

function fail(message) {
  console.error(message);
  process.exit(1);
}
