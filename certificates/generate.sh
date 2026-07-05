# Generates a self-signed certificate valid for 14 days, to use for WebTransport.
#
# The repo includes a pre-generated self-signed certificate and digest, so this
# script is not needed for the usual local workflow while that certificate is
# valid. It is kept as a reference for replacing those files when the
# certificate expires or you want to regenerate it.
#
# Run this from the root folder.
OUT=certificates
mkdir -p "$OUT"
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -keyout "$OUT/key.pem" -out "$OUT/cert.pem" -days 14 -nodes -subj "/CN=localhost" -addext "subjectAltName=DNS:localhost,IP:127.0.0.1"

FINGERPRINT=$(openssl x509 -in "$OUT/cert.pem" -noout -sha256 -fingerprint | sed 's/^.*=//' | sed 's/://g')
printf '%s' "$FINGERPRINT" > "$OUT/digest.txt"

echo "Wrote new fingerprint $FINGERPRINT to $OUT/digest.txt"
