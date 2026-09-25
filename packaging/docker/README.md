# penv image

`ghcr.io/penvhq/penv`: the signed static Linux binary at `/penv`, for amd64 and arm64, built `FROM scratch`. No shell, no libc, no CA bundle: penv carries its own TLS roots.

| Tag | Moves on |
|---|---|
| `1.2.3` | never |
| `1.2`, `1` | every 1.2.x / 1.x release |
| `latest` | every plain release |
| `next` | every prerelease |

A prerelease never moves `latest`, `1` or `1.2`, so while 1.0 is in prerelease only `next` and exact versions exist.

## Copy into your app image

```dockerfile
COPY --from=ghcr.io/penvhq/penv:next /penv /usr/local/bin/penv
ENTRYPOINT ["penv", "run", "--"]
CMD ["node", "server.js"]
```

Build-time values and credentials per platform: [Deploy with Docker](https://penv.cloud/docs/deploy/docker). The entrypoint is [`penv run`](https://penv.cloud/docs/cli/run).

## Run against a mounted project

```bash
docker run --rm -v "$PWD:/work" -w /work ghcr.io/penvhq/penv:next check
docker run --rm -v "$PWD:/work" -w /work ghcr.io/penvhq/penv:next scan
```

The image's entrypoint is `/penv` and it runs as uid 65532, so your mounted folder must be readable by that uid. Commands: [`penv check`](https://penv.cloud/docs/cli/check), [`penv scan`](https://penv.cloud/docs/cli/scan).

## Trust a TLS-inspecting proxy

```bash
docker run --rm -v /etc/ssl/certs/ca-certificates.crt:/ca.pem:ro -e SSL_CERT_FILE=/ca.pem \
  -v "$PWD:/work" -w /work ghcr.io/penvhq/penv:next pull
```

`SSL_CERT_FILE` replaces the Mozilla roots built into penv with your bundle ([environment variables](https://penv.cloud/docs/cli/environment-variables)). In an agent session penv refuses a bundle you can write; a distribution's own root-owned trust store passes. [`penv pull`](https://penv.cloud/docs/cli/pull).

## Build

| Step | File |
|---|---|
| Runs after the release is published | `image` job in `.github/workflows/release.yml` |
| Fetches each binary through `install.sh`; stops unless both the release signature and the digest check | [`fetch.sh`](./fetch.sh) |
| Copies the binary into `scratch`; nothing is compiled | [`Dockerfile`](./Dockerfile) |
