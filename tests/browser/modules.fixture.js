import fs from 'node:fs/promises';
import path from 'node:path';
import { promisify } from 'node:util';
import { execFile, spawn } from 'node:child_process';

const run = promisify(execFile);
const chapters = [
  { id: 'ch01', number: '第1章', english: 'Chapter one' },
  { id: 'ch02', number: '第2章', english: 'Chapter two' },
  { id: 'ch03', number: '第3章', english: 'Chapter three' },
];

function chapterProgram(index) {
  const chapter = chapters[index];
  const next = chapters[index + 1];
  const increment = {
    id: 'enter.increment',
    operation: {
      type: 'assign',
      target: 'visit_count',
      value: {
        type: 'binary',
        op: 'add',
        left: { type: 'var', name: 'visit_count' },
        right: { type: 'const', value: { type: 'i32', value: 1 } },
      },
    },
  };
  const blocks = {
    enter: {
      ops: [increment],
      terminator: { type: 'activate', cue: 'line', next: 'wait_line' },
    },
    wait_line: {
      ops: [],
      terminator: {
        type: 'await',
        conditions: [{ task: 'line', milestone: { type: 'finished' } }],
        next: next ? 'call_next' : 'end',
        on_cancelled: 'end',
        on_failed: 'end',
      },
    },
    end: { ops: [], terminator: { type: 'end', outcome: 'modules-complete' } },
  };
  if (next) {
    blocks.call_next = {
      ops: [],
      terminator: { type: 'call', function: `${next.id}.start`, args: {}, next: 'end' },
    };
  }
  const scenes = {
    station: [{ id: 'background', asset: 'bg.station', x: 0, y: 0, width: 1280, height: 720 }],
  };
  return {
    fragment_format: 1,
    functions: {
      main: {
        entry: 'enter',
        blocks,
      },
    },
    scenes,
    cues: {
      line: {
        effects: [{
          id: 'stage',
          scope: 'scene',
          effect: { type: 'stage_present', scene: 'station', duration_us: '0' },
        }, {
          id: 'line',
          scope: 'interaction',
          effect: { type: 'dialogue', text: 'line', speaker: '', reveal_us: '1000' },
        }],
      },
    },
  };
}

function moduleToml(id) {
  return `id = "${id}"
module_format = 1
sources = ["story.nir.json"]
text_contracts = "texts/contracts.json"
text_revisions = "texts/revisions.json"

[exports]
start = "main"

[text_bundles]
en = "texts/en.json"
zh-Hans = "texts/zh-Hans.json"
`;
}

function textSource(locale, chapter) {
  const text = locale === 'en'
    ? `${chapter.english}: the shared road continues.`
    : `${chapter.number} 雨`;
  return {
    line: {
      source_revision: 1,
      contract_revision: 1,
      spans: [{ type: 'text', id: 'body', text, emphasis: false }],
    },
  };
}

async function writeJson(file, value) {
  await fs.mkdir(path.dirname(file), { recursive: true });
  await fs.writeFile(file, `${JSON.stringify(value, null, 2)}\n`);
}

async function copyProject(project) {
  await fs.mkdir(path.dirname(project), { recursive: true });
  await fs.mkdir(project, { recursive: true });
  await fs.cp(path.resolve('examples/rain-letters'), project, {
    recursive: true,
    filter(source) {
      const relative = path.relative(path.resolve('examples/rain-letters'), source);
      return !relative.split(path.sep).some(part => ['dist', 'reports', '.nir', 'game.lock'].includes(part));
    },
  });
}

async function setInputs(project,prefetchContent) {
  const file = path.join(project, 'game.toml');
  const content = await fs.readFile(file, 'utf8');
  const replacement = `shared = ["content/shared/story.nir.json"]
modules = ["content/ch01/module.toml", "content/ch02/module.toml", "content/ch03/module.toml"]`;
  const updated = content.replace(/^modules\s*=\s*\[[^\n]*\]/m, replacement)
    .replace('title_scene = "station"', 'title_scene = "ch01.station"');
  if (updated === content) throw new Error('could not replace game.toml module list');
  await fs.writeFile(file, updated);
  const playerFile=path.join(project,'config/player.toml');
  const player=await fs.readFile(playerFile,'utf8');
  const configured=player.replace(/^prefetch_content\s*=\s*(?:true|false)\s*$/m,`prefetch_content = ${prefetchContent}`);
  if(configured===player&&!/^prefetch_content\s*=\s*(?:true|false)\s*$/m.test(player))
    throw new Error('could not set config/player.toml prefetch_content');
  await fs.writeFile(playerFile,configured);
}

export async function buildModulesFixture({prefetchContent=false,port=4191}={}) {
  const cli = path.resolve('dist/novelc');
  await fs.mkdir(path.resolve('target/tmp'), { recursive: true });
  const temp = await fs.mkdtemp(path.resolve('target/tmp/nir-modules-'));
  const project = path.join(temp, 'story');
  const web = path.join(temp, 'web');
  await copyProject(project);
  await setInputs(project,prefetchContent);

  await writeJson(path.join(project, 'content/shared/story.nir.json'), {
    fragment_format: 1,
    variables: { visit_count: { type: 'i32', value: 0 } },
  });
  for (const [index, chapter] of chapters.entries()) {
    const base = path.join(project, 'content', chapter.id);
    await fs.mkdir(base, { recursive: true });
    await fs.writeFile(path.join(base, 'module.toml'), moduleToml(chapter.id));
    await writeJson(path.join(base, 'story.nir.json'), chapterProgram(index));
    await writeJson(path.join(base, 'texts/contracts.json'), {
      line: { source_revision: 1, contract_revision: 1, meaning_revision: 1, gates: [], params: {} },
    });
    await writeJson(path.join(base, 'texts/revisions.json'), {
      format: 1,
      source_locale: 'zh-Hans',
      texts: {},
    });
    for (const locale of ['zh-Hans', 'en']) {
      await writeJson(path.join(base, `texts/${locale}.json`), textSource(locale, chapter));
    }
  }

  for (const chapter of chapters) {
    const id = `${chapter.id}.line`;
    await run(cli, ['-p', project, 'text', 'update', '--id', id, '--meaning', 'preserve']);
    await run(cli, ['-p', project, 'text', 'review', '--id', id, '--locale', 'en']);
  }
  await run(cli, ['-p', project, 'resolve']);
  await run(cli, ['-p', project, 'check', '--locked']);
  await run(cli, ['-p', project, 'build', '--locked', '--out', web]);
  const release = await readRelease(web);
  const server = spawn(cli, ['serve', web, '--port', String(port)], { stdio: 'ignore' });
  const origin = `http://127.0.0.1:${port}`;
  try {
    let ready = false;
    for (let attempt = 0; attempt < 100; attempt++) {
      try {
        const response = await fetch(`${origin}/channels/stable.json`);
        if (response.ok) { ready = true; break; }
      } catch {}
      await new Promise(resolve => setTimeout(resolve, 100));
    }
    if (!ready) throw new Error(`module fixture server failed to start at ${origin}`);
  } catch (error) {
    server.kill('SIGTERM');
    throw error;
  }
  return { cli, project, temp, web, origin, server, ...release };
}

export async function readRelease(web) {
  const channel = JSON.parse(await fs.readFile(path.join(web, 'channels/stable.json'), 'utf8'));
  const manifest = JSON.parse(await fs.readFile(path.join(web, `releases/${channel.release}.json`), 'utf8'));
  const rootBytes = await fs.readFile(path.join(web, manifest.objects[manifest.program].path));
  const executable = JSON.parse(rootBytes.toString('utf8'));
  return { channel, manifest, executable, program: executable.program };
}

export async function closeModulesFixture(fixture) {
  if (fixture.server?.exitCode === null && fixture.server?.signalCode === null) {
    const exited = new Promise(resolve => fixture.server.once('exit', resolve));
    fixture.server.kill('SIGTERM');
    await exited;
  }
  await fs.rm(fixture.temp, { recursive: true, force: true });
}

export function moduleObjectHashes(program) {
  return Object.fromEntries(Object.entries(program.modules).map(([id, index]) => [id, {
    code: index.code,
    static: index.static_content,
    locales: index.locales,
  }]));
}

export function catalogObjectHashes(program) {
  return Object.fromEntries(Object.entries(program.catalogs || {}));
}

export function moduleContentHashes(program) {
  return new Set(Object.values(moduleObjectHashes(program)).flatMap(value =>
    [value.code, value.static, ...Object.values(value.locales)].filter(Boolean)));
}

export function runtimeAssetsForLocales(program,uiLocale,textLocale) {
  const ids=new Set((program.title_nodes||[]).map(node=>node.asset).filter(Boolean));
  for(const id of program.locale_config?.ui?.[uiLocale]?.fonts||[])ids.add(id);
  for(const id of program.locale_config?.text?.[textLocale]?.fonts||[])ids.add(id);
  return ids;
}

export function catalogHashesForAssets(program,assetIds) {
  const catalogs=new Set([...assetIds].map(id=>program.assets[id]?.catalog).filter(Boolean));
  return new Set([...catalogs].map(id=>program.catalogs?.[id]).filter(Boolean));
}

export function mediaHashesForAssets(program,assetIds) {
  return new Set([...assetIds].map(id=>program.assets[id]?.object).filter(Boolean));
}

const objectRequests = new WeakMap();
export function trackObjectRequests(page) {
  let objects=objectRequests.get(page);
  if(objects)return objects;
  objects=new Set();
  page.on('request',request=>{
    const match=new URL(request.url()).pathname.match(/\/objects\/([0-9a-f]{64})\./);
    if(match)objects.add(match[1]);
  });
  objectRequests.set(page,objects);
  return objects;
}

export function requestedNetworkObjects(page) {
  return objectRequests.get(page)||new Set();
}

export function resetNetworkObjects(page) {
  objectRequests.get(page)?.clear();
}

export function requestedModuleObjects(page) {
  return page.evaluate(() => window.__nir.diagnostics().events
    .filter(event => event.stage === 'module_requested')
    .map(event => event.object));
}

export { chapters };
