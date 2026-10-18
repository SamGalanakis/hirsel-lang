`ca.cert`, `end.fullchain` and `end.key` are public test fixtures copied from rustls test-ca/rsa-2048 (third-party/NOTICES.md); they secure nothing.

`server.cert` and `server.key` are a self-signed ECDSA P-256 identity for `localhost` and `127.0.0.1`, and `other.key` an unrelated P-256 key, generated for Hirsel's server TLS tests with:

```sh
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -keyout server.key \
  -out server.cert -days 36500 -subj "/CN=localhost" \
  -addext "subjectAltName=DNS:localhost,IP:127.0.0.1" \
  -addext "basicConstraints=critical,CA:FALSE" -addext "keyUsage=critical,digitalSignature" \
  -addext "extendedKeyUsage=serverAuth"
openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out other.key
```

The same identity is embedded in the server TLS conformance programs. It secures nothing.
