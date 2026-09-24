// Written by penv run and loaded before the app (Node --require, Bun --preload,
// Deno --preload). It masks this run's sensitive values in two places the output
// pipe never sees: text handed to console, which in-process log shippers read,
// and bodies the app serves over HTTP. It never touches outbound requests: a
// secret in an Authorization header to its own API is doing its job.
// Key names arrive in PENV_SENSITIVE; values are read from this process's own
// environment, so no second copy of a secret exists. Any failure leaves the app
// as it was.
(function penvPreload() {
  "use strict";
  const g = globalThis;
  if (g.__penvPreload) return;
  g.__penvPreload = true;
  const MASK = "\u2592\u2592\u2592\u2592\u2592\u2592";
  const MIN = 4;
  try {
    // Under Deno, reading a variable the app was not granted would put a
    // permission prompt in front of the person: only granted ones are read.
    const read = (name) => {
      try {
        if (g.Deno && g.Deno.env) {
          const q = g.Deno.permissions && g.Deno.permissions.querySync;
          if (!q || q({ name: "env", variable: name }).state !== "granted") return undefined;
          return g.Deno.env.get(name);
        }
        if (typeof process !== "undefined" && process.env) return process.env[name];
      } catch (_) {}
      return undefined;
    };
    const names = String(read("PENV_SENSITIVE") || "").split(",").filter(Boolean);
    const b64 = (s) => {
      try {
        if (typeof Buffer !== "undefined") return Buffer.from(s, "utf8").toString("base64");
        return btoa(unescape(encodeURIComponent(s)));
      } catch (_) {
        return null;
      }
    };
    const hex = (s) => {
      let out = "";
      for (const b of new TextEncoder().encode(s)) out += b.toString(16).padStart(2, "0");
      return out;
    };
    const forms = [];
    for (const name of names) {
      const v = read(name);
      if (typeof v !== "string" || v.length < MIN) continue;
      for (const f of [v, encodeURIComponent(v), b64(v), b64(v) && b64(v).replace(/=+$/, ""), hex(v), JSON.stringify(v).slice(1, -1)]) {
        if (f && f.length >= MIN && !forms.includes(f)) forms.push(f);
      }
    }
    if (forms.length === 0) return;
    forms.sort((a, b) => b.length - a.length);
    const maskText = (text) => {
      if (typeof text !== "string" || text.length < MIN) return text;
      let out = text;
      for (const f of forms) if (out.includes(f)) out = out.split(f).join(f.slice(0, 2) + MASK);
      return out;
    };
    // Served bodies keep their byte length, so a Content-Length the app already
    // set still matches: two bytes kept, the rest replaced with "*".
    const enc = new TextEncoder();
    const hasBuffer = typeof Buffer !== "undefined";
    const byteForms = forms.map((f) => (hasBuffer ? Buffer.from(f, "utf8") : enc.encode(f)));
    // Buffer's native search where there is one; a byte loop only without it.
    const indexOf = hasBuffer
      ? (hay, needle, from) => Buffer.from(hay.buffer, hay.byteOffset, hay.byteLength).indexOf(needle, from)
      : (hay, needle, from) => {
          outer: for (let i = from; i <= hay.length - needle.length; i++) {
            for (let j = 0; j < needle.length; j++) if (hay[i + j] !== needle[j]) continue outer;
            return i;
          }
          return -1;
        };
    const maskBytes = (bytes) => {
      let out = null;
      for (const f of byteForms) {
        let at = indexOf(out || bytes, f, 0);
        while (at !== -1) {
          if (!out) out = new Uint8Array(bytes);
          out.fill(42, at + 2, at + f.length);
          at = indexOf(out, f, at + f.length);
        }
      }
      return out || bytes;
    };
    const maskChunk = (chunk) => {
      if (typeof chunk === "string") {
        const bytes = enc.encode(chunk);
        const out = maskBytes(bytes);
        return out === bytes ? chunk : (typeof Buffer !== "undefined" ? Buffer.from(out) : out);
      }
      if (chunk instanceof Uint8Array) {
        const out = maskBytes(chunk);
        if (out === chunk) return chunk;
        return typeof Buffer !== "undefined" && Buffer.isBuffer(chunk) ? Buffer.from(out) : out;
      }
      if (chunk instanceof ArrayBuffer) return maskBytes(new Uint8Array(chunk)).buffer;
      return chunk;
    };

    // console: strings masked in place; an Error is handed on as a copy with
    // its message, stack and own fields masked; any other object that would
    // print a secret is handed on as its masked JSON, or, for what JSON cannot
    // show (a Map, a Set, a class instance), its masked inspection. Log shippers
    // (Sentry, Datadog, pino transports) wrap console after the app starts; each
    // method is an accessor, so whatever is assigned later is wrapped too and
    // receives masked arguments.
    let inspect = null;
    try {
      const util =
        typeof require === "function"
          ? require("util")
          : typeof process !== "undefined" && process.getBuiltinModule && process.getBuiltinModule("node:util");
      if (util && typeof util.inspect === "function") inspect = util.inspect;
    } catch (_) {}
    // A plain object as masked JSON, anything JSON cannot show as its masked
    // inspection; the object itself when neither holds a value.
    const maskObject = (a) => {
      try {
        const json = JSON.stringify(a);
        if (typeof json === "string" && maskText(json) !== json) return maskText(json);
      } catch (_) {}
      if (inspect) {
        try {
          const shown = inspect(a);
          if (maskText(shown) !== shown) return maskText(shown);
        } catch (_) {}
      }
      return a;
    };
    const maskField = (v, depth) => {
      if (typeof v === "string") return maskText(v);
      if (v instanceof Error) return depth < 4 ? maskError(v, depth + 1) : v;
      if (v && typeof v === "object") return maskObject(v);
      return v;
    };
    const maskError = (e, depth) => {
      const message = maskText(e.message);
      const stack = maskText(e.stack);
      const cause = e.cause instanceof Error && depth < 4 ? maskError(e.cause, depth + 1) : e.cause;
      let changed = message !== e.message || stack !== e.stack || cause !== e.cause;
      const own = {};
      for (const k of Object.keys(e)) {
        if (k === "cause") continue;
        // An axios error carries config.headers.Authorization a level down.
        own[k] = maskField(e[k], depth);
        if (own[k] !== e[k]) changed = true;
      }
      if (!changed) return e;
      const copy = new Error(message);
      Object.setPrototypeOf(copy, Object.getPrototypeOf(e));
      Object.defineProperty(copy, "stack", { value: stack, writable: true, configurable: true });
      if (cause !== undefined) Object.defineProperty(copy, "cause", { value: cause, writable: true, configurable: true });
      return Object.assign(copy, own);
    };
    const maskArgs = (args) =>
      args.map((a) => {
        if (typeof a === "string") return maskText(a);
        if (!a || typeof a !== "object") return a;
        if (a instanceof Error) return maskError(a, 0);
        return maskObject(a);
      });
    const guard = (fn) => {
      if (typeof fn !== "function" || fn.__penvMasked) return fn;
      const wrapped = function penvConsole(...args) {
        try {
          args = maskArgs(args);
        } catch (_) {}
        return fn.apply(this, args);
      };
      wrapped.__penvMasked = true;
      return wrapped;
    };
    for (const level of ["log", "info", "warn", "error", "debug", "trace"]) {
      let current = guard(console[level]);
      try {
        Object.defineProperty(console, level, {
          configurable: true,
          enumerable: true,
          get() {
            return current;
          },
          set(fn) {
            current = guard(fn);
          },
        });
      } catch (_) {
        console[level] = current;
      }
    }

    // The chunk the HTTP layer just masked, which it hands straight to the
    // socket: that write is not searched a second time.
    let vetted;
    // Node and Bun's node:http, Deno's node:http compatibility: bodies the app serves.
    const patchHttp = (http) => {
      const proto = http && http.ServerResponse && http.ServerResponse.prototype;
      if (!proto || proto.__penv) return;
      proto.__penv = true;
      const write = proto.write;
      const end = proto.end;
      proto.write = function (chunk, ...rest) {
        try {
          chunk = vetted = maskChunk(chunk);
        } catch (_) {}
        try {
          return write.call(this, chunk, ...rest);
        } finally {
          vetted = undefined;
        }
      };
      proto.end = function (chunk, ...rest) {
        try {
          if (chunk !== undefined && typeof chunk !== "function") chunk = vetted = maskChunk(chunk);
        } catch (_) {}
        try {
          return end.call(this, chunk, ...rest);
        } finally {
          vetted = undefined;
        }
      };
    };
    // Below HTTP: every write on a connection a server accepted, so a raw
    // res.socket.write, a WebSocket frame or HTTPS plaintext is covered too.
    // Lengths are kept, so framing and Content-Length stay valid.
    const patchNet = (net) => {
      const proto = net && net.Socket && net.Socket.prototype;
      if (!proto || proto.__penv) return;
      proto.__penv = true;
      const write = proto.write;
      proto.write = function (chunk, ...rest) {
        try {
          if (this.server && (chunk !== vetted || vetted === undefined)) chunk = maskChunk(chunk);
        } catch (_) {}
        return write.call(this, chunk, ...rest);
      };
    };
    try {
      if (typeof require === "function") patchNet(require("net"));
    } catch (_) {}
    try {
      if (typeof process !== "undefined" && process.getBuiltinModule) patchNet(process.getBuiltinModule("node:net"));
    } catch (_) {}
    try {
      if (typeof require === "function") patchHttp(require("http"));
    } catch (_) {}
    try {
      if (typeof process !== "undefined" && process.getBuiltinModule) patchHttp(process.getBuiltinModule("node:http"));
    } catch (_) {}

    // Web Response: Bun.serve, Deno.serve and every fetch-style server build one.
    const Base = g.Response;
    if (typeof Base === "function" && !Base.__penv) {
      const maskBody = (body) => {
        if (body == null) return body;
        if (typeof body === "string" || body instanceof Uint8Array || body instanceof ArrayBuffer) return maskChunk(body);
        if (typeof ReadableStream !== "undefined" && body instanceof ReadableStream) {
          return body.pipeThrough(new TransformStream({ transform(c, ctl) { ctl.enqueue(maskChunk(c)); } }));
        }
        return body;
      };
      class PenvResponse extends Base {
        constructor(body, init) {
          super(maskBody(body), init);
        }
        static json(data, init) {
          const headers = new Headers((init && init.headers) || {});
          if (!headers.has("content-type")) headers.set("content-type", "application/json");
          return new PenvResponse(JSON.stringify(data), { ...init, headers });
        }
      }
      PenvResponse.__penv = true;
      g.Response = PenvResponse;
    }
  } catch (_) {
    // Never break the app.
  }
})();
