# penv image

`ghcr.io/penvhq/penv`: the signed static Linux binary at `/penv`, for amd64 and arm64. It has no shell, no libc and no CA bundle; penv carries its own TLS roots.

| Tag | Moves on |
|---|---|
| `1.2.3` | never |
| `1.2`, `1` | every 1.2.x / 1.x release |
| `latest` | every release that is not a prerelease |
| `next` | every prerelease |

## In an app image

```dockerfile
COPY --from=ghcr.io/penvhq/penv:1 /penv /usr/local/bin/penv
ENTRYPOINT ["penv", "run", "--"]
CMD ["node", "server.js"]
```

The full recipe, with build-time values and credentials per platform, is in [Design, deploying](../../docs/Design.md#deploying).

## On its own

```bash
docker run --rm -v "$PWD:/work" -w /work ghcr.io/penvhq/penv:1 check
docker run --rm -v "$PWD:/work" -w /work ghcr.io/penvhq/penv:1 scan
```

It runs as uid 65532, so the mounted folder must be readable by it.

## Behind a TLS-inspecting proxy

penv trusts the Mozilla roots built into it. A network that re-signs TLS with its own authority needs the bundle that trusts it:

```bash
docker run --rm -v /etc/ssl/certs/ca-certificates.crt:/ca.pem:ro -e SSL_CERT_FILE=/ca.pem \
  -v "$PWD:/work" -w /work ghcr.io/penvhq/penv:1 pull
```

In an agent session penv refuses a bundle the user can write, because an agent could have written it. A distribution's own root-owned trust store is the exception.

## How it is built

The release workflow's `image` job runs after the release is published. [`fetch.sh`](./fetch.sh) takes each binary through `install.sh`, which refuses one whose checksum file is not signed by a release key or whose digest does not match. The [`Dockerfile`](./Dockerfile) copies it into `scratch`. Nothing is compiled for the image, so the image holds the file the release signed, byte for byte.
