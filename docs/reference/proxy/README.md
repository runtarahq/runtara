# Reverse proxy reference configs

RUNTARA in `AUTH_PROVIDER=trust_proxy` mode delegates authentication to a reverse proxy that sits in front of it. This directory collects reference configs for that topology.

## Trust model

The trust boundary moves to the proxy. RUNTARA accepts any request that reaches it and takes the end-user identity from a forwarded header at face value. That is safe only if **all** of the following hold:

1. RUNTARA binds to a loopback address (`127.0.0.1` / `::1` / `localhost`). The server enforces this on startup.
2. The proxy is the only process that can reach that loopback listener. On shared hosts, isolate via namespaces, a private container network, or firewall rules.
3. The proxy **strips every client-supplied copy of `Authorization`, `X-Forwarded-*`, and `X-User-*` before setting its own values**. Forgetting this lets a client send an arbitrary `X-Forwarded-User` and impersonate any user.
4. The `X-Forwarded-User` value the proxy injects comes from a token it validated itself — never from the inbound request.

If you cannot guarantee all four points, use `AUTH_PROVIDER=oidc` instead.

## Files

| File | Use with |
|---|---|
| [`nginx-trust-proxy.conf`](nginx-trust-proxy.conf) | nginx 1.18+ with `ngx_http_auth_jwt_module` or `lua-resty-openidc` |
| [`Caddyfile-trust-proxy`](Caddyfile-trust-proxy) | Caddy 2.7+ with the [`caddy-security`](https://authcrunch.com/docs/authenticate/getting-started) or [`jwt`](https://github.com/greenpau/caddy-security) plugin |

Both configs are starting points — they show the header-strip pattern, the JWT-validation hook, and the upstream proxy. Fill in your IdP's JWKS URI, issuer, and audience before deploying.

## oauth2-proxy

If you want a plug-and-play SSO layer without writing proxy config, run [oauth2-proxy](https://oauth2-proxy.github.io/oauth2-proxy/) in front of RUNTARA and point it at your IdP. A systemd unit template will land in a follow-up change.
