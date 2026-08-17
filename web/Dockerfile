# alien.hartle.tech — the landing site and the installer it serves.
#
# Build context is the REPOSITORY ROOT, not this directory:
#
#     docker build -f web/Dockerfile -t alien-site .
#
# because the installer served at /install.sh is packaging/install.sh, the same
# file the GitHub release attaches. Copying it in at build time rather than
# keeping a second copy under web/ means the site and the release can never
# drift apart — there is one installer, in one place.

FROM caddy:2-alpine

# Drop CAP_NET_BIND_SERVICE from the caddy binary. Under the cluster's
# securityContext (drop:[ALL] + allowPrivilegeEscalation:false +
# runAsNonRoot:true) an fcap-bearing binary fails to exec with EPERM under
# NoNewPrivs. We listen on :8080, so no low-port bind is needed anyway.
USER root
RUN setcap -r /usr/bin/caddy || true

WORKDIR /srv

COPY web/ /srv/
COPY packaging/install.sh /srv/install.sh

# Config is not content.
RUN rm -f /srv/Caddyfile /srv/Dockerfile /srv/CREDITS.md \
    && find /srv -name '._*' -delete \
    && chmod 0644 /srv/install.sh

COPY web/Caddyfile /etc/caddy/Caddyfile

USER nobody
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=3s --start-period=2s \
  CMD wget --quiet --spider http://127.0.0.1:8080/ || exit 1

CMD ["caddy", "run", "--config", "/etc/caddy/Caddyfile", "--adapter", "caddyfile"]
