#!/bin/zsh
# Builds the release app, .nsh/out/Tinycast.app, and signs it with a certificate from the Keychain.
#
# `nsh package` signs macOS apps ad hoc only. An ad-hoc signature changes with every build, and macOS
# keys the Accessibility permission (which pasting needs) to the signature, so the user would grant it
# again after every rebuild; a certificate with a Team ID keeps it.
#
# The identity: MACOS_SIGNING_IDENTITY if set ("-" for ad hoc), else a Developer ID Application or an
# Apple Development certificate in the Keychain, in that order, else ad hoc. Certificates of one name
# (an old one and its renewal) are as good as each other and are picked by hash, which codesign doesn't
# find ambiguous; when the names differ, the build says which it took.
#
# Usage: scripts/build.sh [nsh package options]    e.g. scripts/build.sh --arch universal
set -euo pipefail
cd "${0:A:h}/.."

app=.nsh/out/Tinycast.app

signing_identity() {
  if (( ${+MACOS_SIGNING_IDENTITY} )); then
    print -r -- $MACOS_SIGNING_IDENTITY
    return
  fi
  local out
  out=$(security find-identity -v -p codesigning 2>/dev/null) || { print -- -; return }
  local kind line
  local -a hashes names
  for kind in "Developer ID Application: " "Apple Development: "; do
    hashes=() names=()
    for line in ${(f)out}; do
      if [[ $line =~ '^ *[0-9]+\) ([0-9A-F]{40}) "(.+)"$' && $match[2] == $kind* ]]; then
        hashes+=$match[1]
        names+=$match[2]
      fi
    done
    (( $#hashes )) || continue
    names=(${(u)names})
    if (( $#names > 1 )); then
      print -u2 "Signing with \"$names[1]\", one of:"
      print -u2 -l -- "  "${^names}
      print -u2 "Set MACOS_SIGNING_IDENTITY to choose."
    fi
    print -r -- $hashes[1]
    return
  done
  print -- -
}

nsh package --target macos "$@"

identity=$(signing_identity)
if [[ $identity == - ]]; then
  if [[ ${MACOS_SIGNING_IDENTITY-} == - ]]; then
    print -u2 "MACOS_SIGNING_IDENTITY is \"-\": keeping the ad-hoc signature."
  else
    print -u2 "No signing certificate found: keeping the ad-hoc signature."
  fi
  print -u2 "Accessibility access will have to be granted again after each rebuild."
  exit 0
fi

# The hardened runtime, as notarization needs; a secure timestamp for Developer ID only, so builds
# signed for development don't need the network.
timestamp=--timestamp=none
if [[ $identity == "Developer ID Application: "* ||
      $(security find-certificate -a -c "Developer ID Application" -Z 2>/dev/null) == *$identity* ]]; then
  timestamp=--timestamp
fi
codesign --force --options runtime $timestamp --preserve-metadata=entitlements \
  --sign $identity $app
codesign --verify --strict $app

authority=$(codesign -dvv $app 2>&1 | sed -n 's/^Authority=//p' | head -1)
print "Signed $app with \"$authority\""
