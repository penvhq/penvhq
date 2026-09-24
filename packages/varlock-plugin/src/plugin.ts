import { type Resolver, type PluginCacheAccessor, plugin, resolveCacheTtl } from 'varlock/plugin-lib';
import { createHash } from 'node:crypto';

const { SchemaError, ResolutionError } = plugin.ERRORS;

const PENV_ICON = 'mdi:shield-key-outline';
const DEFAULT_URL = 'https://penv.cloud';
const TIMEOUT_MS = 15_000;

plugin.name = 'penv';
const { debug } = plugin;
// Read while the plugin context is active; it is gone by the time values resolve.
const VERSION = plugin.version;
debug('init - version =', VERSION);
let pluginCache: PluginCacheAccessor | undefined;
try {
  pluginCache = plugin.cache;
} catch {
  // cache unavailable in this runtime context
}
plugin.icon = PENV_ICON;
plugin.standardVars = {
  initDecorator: '@initPenv',
  params: {
    token: { key: 'PENV_TOKEN', dataType: 'penvToken' },
  },
};

/**
 * Values keyed by name. The provider controls the names, and `__proto__` is a
 * valid key, so this never inherits from Object.prototype and is only ever read
 * through an own-property check.
 */
type Values = Record<string, string | undefined>;

function valuesMap(): Values {
  return Object.create(null) as Values;
}

/** A plain JSON value (from the API or the cache) copied into a null-prototype map. */
function toValues(source: unknown): Values {
  const out = valuesMap();
  if (source && typeof source === 'object') {
    for (const [name, value] of Object.entries(source)) {
      out[name] = typeof value === 'string' ? value : undefined;
    }
  }
  return out;
}

type Environment = { org: string, project: string, environment: string };
type Address = Environment & { key: string };

/** `org/project` from `@penv=`, the same header the penv CLI reads. */
let header: { org: string, project: string } | undefined;

type EnvBody = {
  keys?: Array<{ path?: string, name?: unknown, value?: unknown }>,
};

/** https everywhere but loopback, and no user info in front of the host. */
function checkedUrl(raw: string): string {
  let url: URL;
  try {
    url = new URL(raw);
  } catch {
    throw new SchemaError(`penv url "${raw}" is not a URL`);
  }
  const loopback = ['localhost', '127.0.0.1', '[::1]'].includes(url.hostname);
  if (url.protocol !== 'https:' && !(url.protocol === 'http:' && loopback)) {
    throw new SchemaError(`penv url must be https (http only for localhost): ${url.origin}`);
  }
  if (url.username || url.password) {
    throw new SchemaError('penv url must not carry a user name or password');
  }
  return url.origin + url.pathname.replace(/\/+$/, '');
}

function label(at: Environment) {
  return `${at.org}/${at.project}/${at.environment}`;
}

class PenvInstance {
  environment = 'development';
  url = DEFAULT_URL;
  cacheTtl?: string | number;
  private token?: string;
  private org?: string;
  private project?: string;
  private reads = new Map<string, Promise<Values>>();

  set(opts: { environment?: unknown, url?: unknown, token?: unknown, org?: unknown, project?: unknown }) {
    if (typeof opts.environment === 'string' && opts.environment) this.environment = opts.environment;
    if (typeof opts.url === 'string' && opts.url) {
      const url = checkedUrl(opts.url);
      // A committed schema can be changed in a pull request, and CI would then
      // send PENV_TOKEN wherever it points. Another root receives the token only
      // when PENV_URL names it too, the rule the penv CLI follows.
      const chosen = process.env.PENV_URL ? checkedUrl(process.env.PENV_URL) : undefined;
      if (url !== DEFAULT_URL && url !== chosen) {
        throw new SchemaError(`penv url ${url} is not penv.cloud, so the token is not sent there`, {
          tip: `Set PENV_URL=${url} as well to send it on purpose, or remove url= to use penv.cloud`,
        });
      }
      this.url = url;
    }
    if (typeof opts.token === 'string' && opts.token.trim()) this.token = opts.token.trim();
    if (typeof opts.org === 'string' && opts.org) this.org = opts.org;
    if (typeof opts.project === 'string' && opts.project) this.project = opts.project;
  }

  /** The org and project `@initPenv()` or `@penv=` names; `written` is only for the error. */
  private home(written: string) {
    const org = this.org ?? header?.org;
    const project = this.project ?? header?.project;
    if (!org || !project) {
      throw new SchemaError(`${written} needs a project`, {
        tip: 'Add # @penv=org/project to the header, or pass org and project to @initPenv()',
      });
    }
    return { org, project };
  }

  /**
   * `KEY`, `env/KEY`, `project/env/KEY` or `org/project/env/KEY`, as the penv CLI
   * reads them. An environment whose name holds a `/` cannot be written here;
   * name it in `@initPenv(environment=...)` or `penvBulk(...)`, which take it whole.
   */
  address(written: string): Address {
    const parts = written.split('/').map((p) => p.trim());
    if (parts.some((p) => !p) || parts.length > 4) {
      throw new SchemaError(`penv(${written}) is not an address`, {
        tip: 'Write penv(), penv(KEY), penv(env/KEY), penv(project/env/KEY) or penv(org/project/env/KEY)',
      });
    }
    const key = parts[parts.length - 1];
    if (parts.length === 4) return { org: parts[0], project: parts[1], environment: parts[2], key };
    const { org, project } = this.home(`penv(${written})`);
    if (parts.length === 3) return { org, project: parts[0], environment: parts[1], key };
    if (parts.length === 2) return { org, project, environment: parts[0], key };
    return { org, project, environment: this.environment, key };
  }

  /** One environment of the home project, its name taken whole. */
  environmentOf(name: string): Environment {
    return { ...this.home(`penvBulk(${name})`), environment: name };
  }

  /** Short hash naming the token, so a cache is never shared between credentials. */
  private scope(at: Environment) {
    const who = createHash('sha256').update(this.token ?? '').digest('hex').slice(0, 12);
    return `${who}:${this.url}:${label(at)}`;
  }

  /** Every value of one environment: once per load, and through the cache when `cacheTtl` is set. */
  async read(at: Environment): Promise<Values> {
    if (this.cacheTtl !== undefined && pluginCache) {
      const cached = await pluginCache.getOrSet(`penv:${this.scope(at)}`, this.cacheTtl, async () => {
        // The cache stores JSON: a plain object with every name as an own property.
        const values = await this.once(at);
        return Object.fromEntries(Object.entries(values));
      });
      return toValues(cached);
    }
    return this.once(at);
  }

  private once(at: Environment): Promise<Values> {
    const key = label(at);
    let pending = this.reads.get(key);
    if (!pending) {
      pending = this.fetchEnvironment(at);
      this.reads.set(key, pending);
      pending.catch(() => this.reads.delete(key));
    }
    return pending;
  }

  private async fetchEnvironment(at: Environment): Promise<Values> {
    if (!this.token) {
      throw new SchemaError('penv token is required', {
        tip: 'Pass token=$PENV_TOKEN to @initPenv() and set PENV_TOKEN',
      });
    }
    const where = label(at);
    const parts = [at.org, at.project, at.environment];
    // URL parsing collapses `.` and `..` (and their %2e forms), so such a name
    // would read another path on the same host.
    if (parts.some((part) => /^\.+$/.test(part))) {
      throw new SchemaError(`penv cannot read ${where}: a name made only of dots is not one`, {
        tip: 'Check the org, project and environment in @penv and the item',
      });
    }
    // Each part is one path segment; an environment named feature/foo stays one.
    const path = parts.map(encodeURIComponent).join('/');
    let response: Response;
    try {
      response = await fetch(`${this.url}/api/v1/envs/${path}`, {
        headers: {
          authorization: `Bearer ${this.token}`,
          accept: 'application/json',
          'user-agent': `penvhq-varlock-plugin/${VERSION}`,
        },
        // A redirect could carry the token to another host.
        redirect: 'error',
        signal: AbortSignal.timeout(TIMEOUT_MS),
      });
    } catch (err: any) {
      const why = err?.name === 'TimeoutError' ? 'timed out' : 'network error or redirect';
      throw new ResolutionError(`penv.cloud could not be read for ${where}: ${why}`, {
        tip: `Check the network, and that ${this.url} is the right url`,
      });
    }
    if (response.status === 401) {
      throw new ResolutionError(`penv.cloud rejected the token for ${where}`, {
        tip: 'Create a machine token in the penv.cloud console and set PENV_TOKEN',
      });
    }
    if (response.status === 403) {
      throw new ResolutionError(`the token may not read ${where}`, {
        tip: 'Give the machine identity this project and environment in the penv.cloud console',
      });
    }
    if (response.status === 404) {
      throw new ResolutionError(`${where} does not exist on penv.cloud`, {
        tip: 'Check the org, project and environment names; `penv project ls` lists them',
      });
    }
    if (!response.ok) {
      throw new ResolutionError(`penv.cloud answered ${response.status} for ${where}`);
    }
    let body: EnvBody;
    try {
      body = await response.json() as EnvBody;
    } catch {
      throw new ResolutionError(`penv.cloud answered ${where} with something that is not JSON`);
    }
    const values = valuesMap();
    for (const key of Array.isArray(body?.keys) ? body.keys : []) {
      if (!key || key.path || typeof key.name !== 'string') continue;
      values[key.name] = typeof key.value === 'string' ? key.value : undefined;
    }
    debug(`read ${Object.keys(values).length} keys from ${where}`);
    return values;
  }

  async value(at: Address): Promise<string> {
    const values = await this.read(at);
    const where = label(at);
    if (!Object.hasOwn(values, at.key)) {
      throw new ResolutionError(`${at.key} is not in ${where}`, {
        tip: `Add it with: penv set ${at.key} --env ${at.environment}`,
      });
    }
    const value = values[at.key];
    if (value === undefined) {
      throw new ResolutionError(`${at.key} has no stored value in ${where}`, {
        tip: `Set one with: penv set ${at.key} --env ${at.environment}`,
      });
    }
    return value;
  }

  /** Every stored value, as the JSON `@setValuesBulk` reads. */
  async bulk(environment: string): Promise<string> {
    const values = await this.read(this.environmentOf(environment));
    // fromEntries defines each name as an own property, `__proto__` included.
    return JSON.stringify(Object.fromEntries(Object.entries(values).filter(([, v]) => v !== undefined)));
  }
}

const instance = new PenvInstance();
let initialised = false;

// The same header the penv CLI reads, so one .env.schema serves both tools.
plugin.registerRootDecorator({
  name: 'penv',
  description: 'The penv.cloud org and project this schema reads: org/project',
  process(decVal) {
    if (!decVal.isStatic || typeof decVal.staticValue !== 'string') {
      throw new SchemaError('@penv= takes a static org/project');
    }
    let value = decVal.staticValue.trim();
    const colon = value.indexOf(':');
    if (colon !== -1) {
      const provider = value.slice(0, colon);
      if (provider !== 'penv') {
        throw new SchemaError(`@penv=${value} names provider "${provider}"; this plugin reads penv.cloud only`);
      }
      value = value.slice(colon + 1);
    }
    const [org, project, ...rest] = value.split('/');
    if (!org || !project || rest.length) {
      throw new SchemaError(`@penv=${value} is not org/project`);
    }
    header = { org, project };
    return {};
  },
});

plugin.registerRootDecorator({
  name: 'initPenv',
  description: 'Configure penv.cloud access for penv() and penvBulk()',
  isFunction: true,
  async process(argsVal) {
    if (initialised) throw new SchemaError('@initPenv() is already set');
    initialised = true;
    const objArgs = argsVal.objArgs ?? {};
    return {
      environmentResolver: objArgs.environment,
      tokenResolver: objArgs.token,
      urlResolver: objArgs.url,
      orgResolver: objArgs.org,
      projectResolver: objArgs.project,
      cacheTtlResolver: objArgs.cacheTtl,
    };
  },
  async execute({
    environmentResolver, tokenResolver, urlResolver, orgResolver, projectResolver, cacheTtlResolver,
  }) {
    // Unresolved values are not errors yet: an instance nobody reads needs none.
    instance.set({
      environment: await environmentResolver?.resolve(),
      token: await tokenResolver?.resolve(),
      url: await urlResolver?.resolve(),
      org: await orgResolver?.resolve(),
      project: await projectResolver?.resolve(),
    });
    const cacheTtl = await resolveCacheTtl(cacheTtlResolver);
    if (cacheTtl !== undefined) instance.cacheTtl = cacheTtl;
  },
});

plugin.registerDataType({
  name: 'penvToken',
  sensitive: true,
  internal: true,
  typeDescription: 'penv.cloud machine token (pck_...)',
  icon: PENV_ICON,
  docs: [{ description: 'penv.cloud API', url: 'https://github.com/penvhq/penvhq/blob/main/docs/Cloud-API.md' }],
});

plugin.registerResolverFunction({
  name: 'penv',
  label: 'Read a value from penv.cloud',
  icon: PENV_ICON,
  argsSchema: { type: 'array', arrayMinLength: 0, arrayMaxLength: 1 },
  process() {
    let written: Resolver | undefined;
    let itemKey: string | undefined;
    if (this.arrArgs?.length) {
      written = this.arrArgs[0];
    } else {
      const parent = (this as any).parent;
      if (!parent || typeof parent.key !== 'string') {
        throw new SchemaError('penv() with no argument reads the key it is on, so it must be on a config item');
      }
      itemKey = parent.key;
    }
    return { written, itemKey };
  },
  async resolve({ written, itemKey }) {
    const address = written ? await written.resolve() : itemKey;
    if (typeof address !== 'string') throw new SchemaError('penv() takes an address written as text');
    return instance.value(instance.address(address));
  },
});

plugin.registerResolverFunction({
  name: 'penvBulk',
  label: 'Load every value of a penv.cloud environment',
  icon: PENV_ICON,
  argsSchema: { type: 'array', arrayMaxLength: 1 },
  process() {
    return { environment: this.arrArgs?.[0] };
  },
  async resolve({ environment }) {
    const name = environment ? await environment.resolve() : instance.environment;
    if (typeof name !== 'string' || !name) throw new SchemaError('penvBulk() takes an environment name');
    return instance.bulk(name);
  },
});
