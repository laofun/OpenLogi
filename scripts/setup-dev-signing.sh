#!/usr/bin/env bash
#
# Create a stable self-signed code-signing certificate ("OpenLogi Dev") so that
# macOS (TCC) keeps the Accessibility / Input Monitoring / Bluetooth grants for
# the dev build across rebuilds.
#
# Why: a bare `cargo build` produces an *ad-hoc* signature, which TCC keys on
# the binary's cdhash. The cdhash changes every build, so each rebuild looks
# like a new app and the granted permissions are dropped. Signing the dev
# bundle with a fixed certificate makes TCC key on (bundle id + certificate)
# instead — both stable — so you grant permissions once.
#
# `scripts/cargo-run-macos.sh` picks this cert up automatically (by name) and
# re-signs the bundle on every `cargo run -p openlogi-gui`.
#
# Run once:  scripts/setup-dev-signing.sh
# Idempotent: re-running detects the existing cert and exits early.
set -euo pipefail

CERT_NAME="OpenLogi Dev"
LOGIN_KC="$HOME/Library/Keychains/login.keychain-db"

if security find-identity -p codesigning 2>/dev/null | grep -q "\"$CERT_NAME\""; then
  echo "==> '$CERT_NAME' code-signing identity already present — nothing to do."
  security find-identity -p codesigning | grep "$CERT_NAME"
  exit 0
fi

echo "==> Creating self-signed code-signing certificate '$CERT_NAME'"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
cd "$WORK"

# A throwaway password: `security import` is unreliable importing a p12 with an
# empty passphrase, so we use a fixed one. The p12 file is deleted on exit.
PW="openlogi-dev"

cat > cs.cnf <<'EOF'
[ req ]
distinguished_name = dn
x509_extensions    = v3
prompt             = no
[ dn ]
CN = OpenLogi Dev
[ v3 ]
basicConstraints   = critical,CA:false
keyUsage           = critical,digitalSignature
extendedKeyUsage   = critical,codeSigning
EOF

# Self-signed leaf cert with the Code Signing EKU, valid 10 years.
/usr/bin/openssl req -x509 -newkey rsa:2048 -nodes \
  -keyout key.pem -out cert.pem -days 3650 -config cs.cnf -sha256 >/dev/null 2>&1

# Bundle into a PKCS#12. The legacy SHA1/3DES algorithms are what the macOS
# Security framework's importer accepts; LibreSSL's modern defaults fail with
# "MAC verification failed".
/usr/bin/openssl pkcs12 -export -inkey key.pem -in cert.pem \
  -name "$CERT_NAME" -out dev.p12 -passout "pass:$PW" \
  -certpbe PBE-SHA1-3DES -keypbe PBE-SHA1-3DES -macalg sha1 >/dev/null 2>&1

# Import key + cert into the login keychain and pre-authorise codesign/security
# to use the private key without an interactive prompt on each build.
security import dev.p12 -k "$LOGIN_KC" -P "$PW" \
  -T /usr/bin/codesign -T /usr/bin/security

echo
echo "==> Done. Identity installed:"
security find-identity -p codesigning | grep "$CERT_NAME" || true
echo
echo "Note: the cert shows as untrusted (CSSMERR_TP_NOT_TRUSTED) — that's fine."
echo "codesign signs with it regardless, and TCC keys grants on it. The next"
echo "'cargo run -p openlogi-gui' will sign the bundle automatically."
echo
echo "First run after this still needs a one-time permission grant (the bundle"
echo "moves from ad-hoc to this identity). Grants persist across rebuilds after."
