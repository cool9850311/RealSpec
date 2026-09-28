#!/usr/bin/env sh
# Generate the TLS certificate this example serves itself over.
#
#   local/certs/generate.sh
#
# WHY THIS IS A SCRIPT AND NOT TWO COMMITTED FILES
# ------------------------------------------------
# A certificate needs a private key, and a private key does not belong in a
# repository — least of all this one, which exists to be copied. A committed
# key also carries whoever generated it: mkcert names the CA after the account
# and the machine, so the artefact would publish a developer's user name and
# hostname to everyone who clones. And it expires, silently, into a TLS
# handshake failure that nobody connects back to a file checked in years
# earlier.
#
# So the repository ships the recipe and .gitignore keeps the output out.
#
# WHAT IT PRODUCES
# ----------------
# cert.pem + key.pem covering the loopback address `127.0.0.1`. Every e2e
# scenario is reached there on its own published port, so one certificate covers
# all of them and no name has to resolve.
#
# mkcert is preferred when present, because `mkcert -install` puts its CA in the
# system trust store and a human browsing https://127.0.0.1 during
# development then gets no warning. mkcert itself prints a reminder when that
# step has not been run, in which case the browser warns exactly as it would for
# the fallback. Where mkcert is absent, a self-signed certificate is equivalent
# for the suite — Playwright runs with ignoreHTTPSErrors, so the tests never
# cared whether the issuer was trusted, only that TLS terminates.
#
# Idempotent: an existing certificate that still covers that address and has not
# expired is left alone.
set -eu

dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
cert="$dir/cert.pem"
key="$dir/key.pem"

# Read the subject-alternative names out of a certificate.
#
# `openssl x509 -ext subjectAltName` is the obvious way and is NOT portable:
# macOS ships LibreSSL as /usr/bin/openssl, which does not know that flag and
# answers with a usage dump — under `set -e` that aborts this script, and the
# caller sees the certificate step fail for a reason that has nothing to do with
# certificates. Parsing -text works on both.
sans() {
  openssl x509 -in "$1" -noout -text 2>/dev/null |
    awk '/X509v3 Subject Alternative Name/ { getline; print; exit }'
}

if [ -f "$cert" ] && [ -f "$key" ]; then
  if openssl x509 -in "$cert" -noout -checkend 604800 >/dev/null 2>&1 &&
    sans "$cert" | grep -q 'IP Address:127\.0\.0\.1'; then
    echo "certs: $cert is present, covers 127.0.0.1 and is not about to expire"
    exit 0
  fi
  echo "certs: existing certificate is expired or does not cover 127.0.0.1 — regenerating"
fi

if command -v mkcert >/dev/null 2>&1; then
  echo "certs: generating with mkcert. If it reports that its CA is not installed,"
  echo "certs: run \`mkcert -install\` to stop the browser warning; tests do not need it."
  mkcert -cert-file "$cert" -key-file "$key" "127.0.0.1"
else
  echo "certs: mkcert not found — generating a self-signed certificate with openssl"
  echo "certs: the e2e suite does not care (it runs with ignoreHTTPSErrors), but a browser will warn."
  echo "certs: install mkcert and re-run this script to silence that."
  openssl req -x509 -newkey rsa:2048 -nodes -days 825 \
    -keyout "$key" -out "$cert" \
    -subj "/CN=127.0.0.1" \
    -addext "subjectAltName=IP:127.0.0.1" >/dev/null 2>&1
fi

chmod 600 "$key"
echo "certs: wrote $cert and $key"
openssl x509 -in "$cert" -noout -subject -enddate
echo "certs: covered: $(sans "$cert" | tr -s ' ' | sed 's/^ //')"
