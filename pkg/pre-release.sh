#!/usr/bin/env bash
# Prepend a new %changelog entry in pkg/fnf.spec for the upcoming release.
# Invoked by cargo-release as pre_release_hook; receives NEW_VERSION.

set -euo pipefail

VERSION="${NEW_VERSION:?}"
DATE=$(date +"%a %b %d %Y")
EMAIL="sebdotv@gmail.com"
SPEC="pkg/fnf.spec"

NEW_ENTRY="* ${DATE} sebdotv <${EMAIL}> - ${VERSION}-1\n- Release ${VERSION}\n"

# Insert the new entry immediately after the %changelog line
sed -i "/^%changelog$/a ${NEW_ENTRY}" "${SPEC}"
