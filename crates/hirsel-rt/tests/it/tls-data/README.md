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

The client-certificate and SNI fixtures are P-256 as well, generated the same way (the CA
and rogue keys were discarded after signing):

- `client-ca.cert`: the self-signed "Hirsel test client CA" (`CA:TRUE`, `keyCertSign`).
- `client.cert` and `client.key`: `/O=Hirsel/CN=agent-7` with
  `subjectAltName=DNS:agent-7.hirsel.test` and `extendedKeyUsage=clientAuth`, issued by
  `client-ca.cert` with serial 2 (`openssl x509 -req -CA client-ca.cert -CAkey ...`).
- `rogue-ca.cert` and `rogue.cert`: a second CA, and the same subject issued by it with
  serial 3, which `client-ca.cert` does not trust.
- `alpha.cert`/`alpha.key` and `beta.cert`/`beta.key`: self-signed server identities for
  `alpha.hirsel.test` and `beta.hirsel.test`, made like `server.cert`.

The client and SNI conformance programs embed the same certificates. They secure nothing.
