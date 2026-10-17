# Test-only keys

`access-test-signer.pem` and `access-test-impostor.pem` are throwaway RSA keys
generated for tests of Cloudflare Access token verification. They protect
nothing. `access-test-jwks.json` is the public half of the signer key, in the
format Cloudflare publishes at `/cdn-cgi/access/certs`.
