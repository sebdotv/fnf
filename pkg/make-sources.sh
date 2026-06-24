#!/usr/bin/env bash
# Produce the two source tarballs required by fnf.spec:
#   Source0: fnf-<version>.tar.gz      (GitHub archive / git export)
#   Source1: fnf-<version>-vendor.tar.gz (cargo vendor snapshot)
#
# Run from the repo root.  Requires: git, cargo, tar.

set -euo pipefail

VERSION=$(grep '^version' Cargo.toml | head -1 | cut -d'"' -f2)
ARCHIVE="fnf-${VERSION}"

echo "==> Version: ${VERSION}"

# Source0 — git archive (matches GitHub's "Download tar.gz" format)
echo "==> Creating ${ARCHIVE}.tar.gz ..."
git archive --prefix="${ARCHIVE}/" HEAD | gzip -n > "pkg/${ARCHIVE}.tar.gz"

# Source1 — vendor snapshot
echo "==> Creating ${ARCHIVE}-vendor.tar.gz ..."
cargo vendor --quiet vendor
tar czf "pkg/${ARCHIVE}-vendor.tar.gz" vendor
rm -rf vendor

echo "==> Done. Files in pkg/:"
ls -lh "pkg/${ARCHIVE}.tar.gz" "pkg/${ARCHIVE}-vendor.tar.gz"
