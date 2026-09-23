// End to end: the built plugin, loaded by the published varlock CLI, against a
// fake penv.cloud. Run with `npm test` (it builds first).
import { test, after } from 'node:test';
import assert from 'node:assert/strict';
import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const PACKAGE = path.join(HERE, '..');
// The schema names the plugin by a path relative to the project, which load()
// fills in; forward slashes read the same on Windows.
const PLUGIN = '{{plugin}}';
// The CLI's own entry, run with this node, so the tests need no shell shim (Windows).
const varlockPackage = JSON.parse(fs.readFileSync(path.join(PACKAGE, 'node_modules', 'varlock', 'package.json'), 'utf8'));
const VARLOCK_BIN = typeof varlockPackage.bin === 'string' ? varlockPackage.bin : varlockPackage.bin.varlock;
const VARLOCK = path.join(PACKAGE, 'node_modules', 'varlock', VARLOCK_BIN);
const TOKEN = 'pck_test_token';

const ENVS = {
  'acme/api/development': [
    { name: 'DATABASE_URL', value: 'postgres://dev' },
    { name: 'NO_VALUE' },
    { name: '__proto__', value: 'proto-secret' },
  ],
  'acme/api/production': [
    { name: 'DATABASE_URL', value: 'postgres://prod' },
    { name: 'STRIPE_KEY', value: 'sk_live_test' },
  ],
  'acme/api/feature/foo': [{ name: 'DATABASE_URL', value: 'postgres://feature-foo' }],
  'acme/feature/foo': [{ name: 'DATABASE_URL', value: 'WRONG: project feature, environment foo' }],
  'acme/billing/production': [{ name: 'API_TOKEN', value: 'billing-token' }],
  'other/shared/production': [{ name: 'SENTRY_DSN', value: 'https://sentry' }],
};

/** A fake penv.cloud. Each path segment is decoded on its own, as the real API must. */
const requests = [];
const server = http.createServer((req, res) => {
  requests.push(req.url);
  if (req.url.startsWith('/redirect/')) {
    res.writeHead(302, { location: 'http://127.0.0.1:1/stolen' });
    res.end();
    return;
  }
  if (req.url.startsWith('/garbage/')) {
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end('<html>not json</html>');
    return;
  }
  if (req.headers.authorization !== `Bearer ${TOKEN}`) {
    res.writeHead(401);
    res.end('{"error":"unauthorized"}');
    return;
  }
  const match = req.url.match(/^\/api\/v1\/envs\/([^/?]+)\/([^/?]+)\/([^/?]+)$/);
  const keys = match && ENVS[match.slice(1).map(decodeURIComponent).join('/')];
  if (!keys) {
    res.writeHead(404);
    res.end('{"error":"not_found"}');
    return;
  }
  res.writeHead(200, { 'content-type': 'application/json' });
  res.end(JSON.stringify({ keys: keys.map((k) => ({ path: '', ...k })) }));
});
await new Promise((resolve) => { server.listen(0, '127.0.0.1', resolve); });
const URL_ROOT = `http://127.0.0.1:${server.address().port}`;
// Projects live under the package: varlock takes a relative @plugin path only on
// Windows, and a relative path cannot cross drives.
fs.mkdirSync(path.join(PACKAGE, '.test-projects'), { recursive: true });
const scratch = fs.mkdtempSync(path.join(PACKAGE, '.test-projects', 'run-'));
after(() => { server.close(); fs.rmSync(scratch, { recursive: true, force: true }); });

const HEADER = ({ url = URL_ROOT, init = '', penv = 'acme/api' } = {}) => [
  `# @plugin(${PLUGIN})`,
  `# @penv=${penv}`,
  `# @initPenv(environment=$APP_ENV, token=$PENV_TOKEN, url="${url}"${init})`,
  '# @currentEnv=$APP_ENV',
  '# ---',
  '# @type=enum(development, production) @sensitive=false',
  'APP_ENV=development',
  '# @type=penvToken',
  'PENV_TOKEN=',
].join('\n');

let count = 0;
/** `varlock load --format json` in a fresh project; the cache lives in the project's own home. */
function load(schema, env = {}, dir) {
  dir ??= fs.mkdtempSync(path.join(scratch, `p${count++}-`));
  const relative = path.relative(dir, PACKAGE).split(path.sep).join('/');
  fs.writeFileSync(path.join(dir, '.env.schema'), schema.replaceAll('{{plugin}}', relative));
  return new Promise((resolve) => {
    const child = spawn(process.execPath, [VARLOCK, 'load', '--format', 'json'], {
      cwd: dir,
      env: {
        // The real home, so the OS's own key store backs varlock's cache where it
        // has one; varlock's directory moves into the project, so runs share nothing.
        ...process.env,
        XDG_CONFIG_HOME: path.join(dir, '.config'),
        // Where varlock falls back to file-based encryption (Linux without a
        // keyring), this key backs the disk cache instead.
        _VARLOCK_CACHE_KEY: '11'.repeat(32),
        // varlock caches differently in CI; the tests describe a developer machine.
        CI: '', GITHUB_ACTIONS: '',
        VARLOCK_TELEMETRY_DISABLED: 'true', PENV_TOKEN: TOKEN, PENV_URL: URL_ROOT, ...env,
      },
    });
    let out = '';
    let stdout = '';
    child.stdout.on('data', (d) => { out += d; stdout += d; });
    child.stderr.on('data', (d) => { out += d; });
    child.on('close', (code) => {
      let values;
      // varlock may print notes (keyring, cache) before the JSON document.
      const start = stdout.indexOf('\n{') === -1 ? stdout.indexOf('{') : stdout.indexOf('\n{') + 1;
      try { values = JSON.parse(stdout.slice(start)); } catch { values = undefined; }
      resolve({ code, out, values, dir });
    });
  });
}
const since = (n) => requests.slice(n);

test('penv() reads the item key in the current environment; the token stays out', async () => {
  const r = await load(`${HEADER()}\nDATABASE_URL=penv()`);
  assert.equal(r.values?.DATABASE_URL, 'postgres://dev', r.out);
  assert.ok(!r.out.includes(TOKEN), 'token printed');
  assert.ok(!('PENV_TOKEN' in r.values), 'penvToken is internal');
});

test('@currentEnv picks the environment', async () => {
  const r = await load(`${HEADER()}\nDATABASE_URL=penv()`, { APP_ENV: 'production' });
  assert.equal(r.values?.DATABASE_URL, 'postgres://prod', r.out);
});

test('addresses reach another key, environment, project and org, one request per environment', async () => {
  const before = requests.length;
  const r = await load([
    HEADER(),
    'STRIPE=penv(production/STRIPE_KEY)',
    'PROD_DB=penv(production/DATABASE_URL)',
    'BILLING=penv(billing/production/API_TOKEN)',
    'SENTRY=penv(other/shared/production/SENTRY_DSN)',
    'RENAMED=penv(DATABASE_URL)',
    'DATABASE_URL=penv()',
  ].join('\n'));
  assert.deepEqual(
    [r.values?.STRIPE, r.values?.PROD_DB, r.values?.BILLING, r.values?.SENTRY, r.values?.RENAMED],
    ['sk_live_test', 'postgres://prod', 'billing-token', 'https://sentry', 'postgres://dev'],
    r.out,
  );
  assert.deepEqual(since(before).sort(), [
    '/api/v1/envs/acme/api/development',
    '/api/v1/envs/acme/api/production',
    '/api/v1/envs/acme/billing/production',
    '/api/v1/envs/other/shared/production',
  ]);
});

test('penvBulk() fills every item for @setValuesBulk', async () => {
  const r = await load([
    `# @plugin(${PLUGIN})`, '# @penv=acme/api', `# @initPenv(token=$PENV_TOKEN, url="${URL_ROOT}")`,
    '# @setValuesBulk(penvBulk(production))', '# ---', '# @type=penvToken', 'PENV_TOKEN=', 'DATABASE_URL=', 'STRIPE_KEY=',
  ].join('\n'));
  assert.equal(r.values?.DATABASE_URL, 'postgres://prod', r.out);
  assert.equal(r.values?.STRIPE_KEY, 'sk_live_test', r.out);
});

test('finding 1: a key named __proto__ is read as an own value, alone and in bulk', async () => {
  const one = await load(`${HEADER()}\nPROTO=penv(__proto__)\nMISSING_PROTO_NEIGHBOUR=penv(constructor)`);
  assert.match(one.out, /constructor is not in acme\/api\/development/, 'an inherited name is not a key');
  const alone = await load(`${HEADER()}\nPROTO=penv(__proto__)`);
  assert.equal(alone.values?.PROTO, 'proto-secret', alone.out);
  const bulk = await load([
    `# @plugin(${PLUGIN})`, '# @penv=acme/api', `# @initPenv(token=$PENV_TOKEN, url="${URL_ROOT}")`,
    '# @setValuesBulk(penvBulk())', '# ---', '# @type=penvToken', 'PENV_TOKEN=', 'DATABASE_URL=',
  ].join('\n'));
  assert.equal(bulk.values?.DATABASE_URL, 'postgres://dev', bulk.out);
});

test('finding 1: the bulk JSON carries __proto__ as its own key', async () => {
  // penvBulk() as an item value shows the JSON @setValuesBulk would receive.
  const r = await load(`${HEADER()}\n# @sensitive=false\nBULK=penvBulk()`);
  const bulk = JSON.parse(r.values?.BULK ?? 'null');
  assert.ok(bulk, r.out);
  assert.equal(Object.hasOwn(bulk, '__proto__'), true, r.values?.BULK);
  assert.equal(bulk.__proto__, 'proto-secret');
  assert.equal(bulk.DATABASE_URL, 'postgres://dev');
  assert.equal(Object.hasOwn(bulk, 'NO_VALUE'), false, 'a key with no value is left out');
});

test('finding 2: an environment named feature/foo is one path segment, in penvBulk and @initPenv', async () => {
  let before = requests.length;
  const bulk = await load([
    `# @plugin(${PLUGIN})`, '# @penv=acme/api', `# @initPenv(token=$PENV_TOKEN, url="${URL_ROOT}")`,
    '# @setValuesBulk(penvBulk("feature/foo"))', '# ---', '# @type=penvToken', 'PENV_TOKEN=', 'DATABASE_URL=',
  ].join('\n'));
  assert.equal(bulk.values?.DATABASE_URL, 'postgres://feature-foo', bulk.out);
  assert.deepEqual(since(before), ['/api/v1/envs/acme/api/feature%2Ffoo']);

  before = requests.length;
  const init = await load([
    `# @plugin(${PLUGIN})`, '# @penv=acme/api', `# @initPenv(environment="feature/foo", token=$PENV_TOKEN, url="${URL_ROOT}")`,
    '# ---', '# @type=penvToken', 'PENV_TOKEN=', 'DATABASE_URL=penv()',
  ].join('\n'));
  assert.equal(init.values?.DATABASE_URL, 'postgres://feature-foo', init.out);
  assert.deepEqual(since(before), ['/api/v1/envs/acme/api/feature%2Ffoo']);
});

test('finding 3: cacheTtl applies to penvBulk(); without it every load reads', async (t) => {
  // On macOS varlock decrypts its cache through a Secure Enclave daemon. On GitHub's
  // macOS runners the daemon fails to bind its socket ("Address already in use", seen
  // with VARLOCK_DEBUG=1), so every load re-reads, whatever the plugin does.
  if (process.platform === 'darwin' && process.env.GITHUB_ACTIONS) {
    t.skip('varlock\'s macOS cache daemon does not start on GitHub runners');
    return;
  }
  const bulkSchema = (init) => [
    `# @plugin(${PLUGIN})`, '# @penv=acme/api', `# @initPenv(token=$PENV_TOKEN, url="${URL_ROOT}"${init})`,
    '# @setValuesBulk(penvBulk(production))', '# ---', '# @type=penvToken', 'PENV_TOKEN=', 'STRIPE_KEY=',
  ].join('\n');
  const dir = fs.mkdtempSync(path.join(scratch, 'cache-'));
  let before = requests.length;
  const first = await load(bulkSchema(', cacheTtl="1h"'), {}, dir);
  // varlock keeps a disk cache only where its key store works. A machine with
  // none falls back to memory, and then nothing outlives one load.
  const cacheDir = path.join(dir, '.config', 'varlock', 'cache');
  if (!fs.existsSync(cacheDir) || fs.readdirSync(cacheDir).length === 0) {
    t.skip(`varlock has no disk cache on this machine (${process.platform})`);
    return;
  }
  const second = await load(bulkSchema(', cacheTtl="1h"'), {}, dir);
  assert.equal(first.values?.STRIPE_KEY, 'sk_live_test', first.out);
  assert.equal(second.values?.STRIPE_KEY, 'sk_live_test', second.out);
  if (since(before).length !== 1) {
    const debug = await load(bulkSchema(', cacheTtl="1h"'), { VARLOCK_DEBUG: '1' }, dir);
    assert.fail(`cached: ${since(before)}\nvarlock debug:\n${debug.out.slice(-3000)}`);
  }

  const dir2 = fs.mkdtempSync(path.join(scratch, 'nocache-'));
  before = requests.length;
  await load(bulkSchema(''), {}, dir2);
  await load(bulkSchema(''), {}, dir2);
  assert.equal(since(before).length, 2, 'no cacheTtl, no cache');

  // A different token never reads another token's cache.
  before = requests.length;
  const other = await load(bulkSchema(', cacheTtl="1h"'), { PENV_TOKEN: 'pck_other' }, dir);
  assert.match(other.out, /rejected the token/);
  assert.equal(since(before).length, 1);
});

test('@penv=penv:org/project reads like org/project; another provider is refused', async () => {
  const ok = await load(`${HEADER({ penv: 'penv:acme/api' })}\nDATABASE_URL=penv()`);
  assert.equal(ok.values?.DATABASE_URL, 'postgres://dev', ok.out);
  const other = await load(`${HEADER({ penv: 'doppler:acme/api' })}\nDATABASE_URL=penv()`);
  assert.notEqual(other.code, 0);
  assert.match(other.out, /names provider "doppler"/);
});

test('a missing key, a key with no value and a rejected token are errors that name the fix', async () => {
  const missing = await load(`${HEADER()}\nMISSING=penv()\nNO_VALUE=penv()`);
  assert.notEqual(missing.code, 0);
  assert.match(missing.out, /MISSING is not in acme\/api\/development/);
  assert.match(missing.out, /NO_VALUE has no stored value/);
  const wrong = await load(`${HEADER()}\nDATABASE_URL=penv()`, { PENV_TOKEN: 'pck_wrong_one' });
  assert.match(wrong.out, /rejected the token/);
  assert.ok(!wrong.out.includes('pck_wrong_one'), 'a rejected token is not printed');
});

test('a redirect is refused, not followed; a body that is not JSON is an error', async () => {
  let before = requests.length;
  const redirect = await load(`${HEADER({ url: `${URL_ROOT}/redirect` })}\nDATABASE_URL=penv()`, { PENV_URL: `${URL_ROOT}/redirect` });
  assert.notEqual(redirect.code, 0);
  assert.equal(since(before).length, 1, 'only the first request was sent');
  before = requests.length;
  const garbage = await load(`${HEADER({ url: `${URL_ROOT}/garbage` })}\nDATABASE_URL=penv()`, { PENV_URL: `${URL_ROOT}/garbage` });
  assert.match(garbage.out, /not JSON/);
});

test('plain http to a remote host, and a URL with a user, are refused before any request', async () => {
  for (const url of ['http://example.com', 'https://user:pw@example.com']) {
    const r = await load(`${HEADER({ url })}\nDATABASE_URL=penv()`);
    assert.notEqual(r.code, 0, url);
    assert.match(r.out, /must be https|must not carry a user/, r.out);
  }
});

test('a url only the schema names never receives the token', async () => {
  const before = requests.length;
  const r = await load(`${HEADER()}\nDATABASE_URL=penv()`, { PENV_URL: '' });
  assert.notEqual(r.code, 0);
  assert.match(r.out, /is not penv.cloud, so the token is not sent there/, r.out);
  assert.equal(since(before).length, 0, 'nothing was sent');
  const chosen = await load(`${HEADER()}\nDATABASE_URL=penv()`, { PENV_URL: URL_ROOT });
  assert.equal(chosen.values?.DATABASE_URL, 'postgres://dev', 'PENV_URL naming the same root sends it');
});
