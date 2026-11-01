#!/bin/bash
# Create a self-signed code signing identity ("Vroom Local Signing") in the login
# keychain. Builds signed with it keep the same identity, so macOS remembers the
# Screen Recording, camera and microphone permissions across rebuilds.
# Only meaningful on this Mac; releases need a real Developer ID.
set -euo pipefail
name="${1:-Vroom Local Signing}"
if security find-certificate -c "$name" >/dev/null 2>&1; then
  echo "$name already exists"
  exit 0
fi
dir="$(mktemp -d)"
trap 'rm -rf "$dir"' EXIT
cat > "$dir/cert.cnf" <<EOF
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = $name
[ext]
basicConstraints = critical,CA:false
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,codeSigning
EOF
openssl req -x509 -newkey rsa:2048 -nodes -days 3650 -config "$dir/cert.cnf" \
  -keyout "$dir/key.pem" -out "$dir/cert.pem" 2>/dev/null
pass="$(openssl rand -hex 16)"
openssl pkcs12 -export -legacy -inkey "$dir/key.pem" -in "$dir/cert.pem" -name "$name" \
  -passout "pass:$pass" -out "$dir/id.p12" 2>/dev/null \
  || openssl pkcs12 -export -inkey "$dir/key.pem" -in "$dir/cert.pem" -name "$name" -passout "pass:$pass" -out "$dir/id.p12"
security import "$dir/id.p12" -k "$HOME/Library/Keychains/login.keychain-db" -P "$pass" -T /usr/bin/codesign >/dev/null
echo "Created $name. Build with: VROOM_SIGN_IDENTITY=\"$name\" make install"
