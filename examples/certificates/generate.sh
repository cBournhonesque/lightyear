# Generates a self-signed certificate valid for 14 days, to use for webtransport
# run this from the root folder
OUT=examples/certificates
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -keyout $OUT/key.pem -out $OUT/cert.pem -days 14 -nodes -subj "/CN=localhost"
