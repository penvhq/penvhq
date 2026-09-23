# The penv image: the signed static binary and nothing else. No shell, no libc,
# no CA bundle (the TLS roots are compiled in). Copy it into an app image:
#
#   COPY --from=ghcr.io/penvhq/penv:1 /penv /usr/local/bin/penv
#
# or run it against a mounted project:
#
#   docker run --rm -v "$PWD:/work" -w /work ghcr.io/penvhq/penv:1 check
#
# The build context is made by fetch.sh, which verifies each binary's release
# signature and digest before it is copied in.
FROM scratch
ARG TARGETARCH
COPY --chmod=0755 bin/${TARGETARCH}/penv /penv
# Not root. A mounted project is read with this user's permissions.
USER 65532:65532
ENTRYPOINT ["/penv"]
