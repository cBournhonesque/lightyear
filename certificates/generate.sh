# Generates a self-signed certificate valid for 14 days, to use for webtransport
# run this from the root folder
OUT=certificates
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -keyout $OUT/key.pem -out $OUT/cert.pem -days 14 -nodes -subj "/CN=localhost"

FINGERPRINT=$(openssl x509 -in "$OUT/cert.pem" -noout -sha256 -fingerprint | sed 's/^.*=//' | sed 's/://g')
printf '%s' "$FINGERPRINT" > $OUT/digest.txt

echo "Wrote new fingerprint $FINGERPRINT to $OUT/digest.txt"