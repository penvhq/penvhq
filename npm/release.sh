#!/usr/bin/env sh
# Point the `latest` dist-tag of every @penvhq CLI package at VERSION.
# Usage: NPM_TOKEN=npm_xxx npm/release.sh [version]   (default: newest penvhq/penvhq GitHub release)
set -eu

: "${NPM_TOKEN:?set NPM_TOKEN to an npm token with publish access}"
VERSION="${1:-$(gh release view -R penvhq/penvhq --json tagName -q .tagName)}"
VERSION="${VERSION#v}"
echo "Tagging @penvhq CLI $VERSION as latest"

for p in cli-linux-x64 cli-linux-arm64 cli-darwin-x64 cli-darwin-arm64 cli-win32-x64 cli-win32-arm64 cli; do
  npm dist-tag add "@penvhq/$p@$VERSION" latest --//registry.npmjs.org/:_authToken="$NPM_TOKEN"
done
