# syntax=docker/dockerfile:1.7
#
# blockmill/drawdb: drawDB (https://github.com/drawdb-io/drawdb, AGPL-3.0)
# served as static files by an unprivileged nginx. The source is unmodified.
#
# The build context is a checkout of drawdb-io/drawdb at DRAWDB_REVISION:
#
#   git clone https://github.com/drawdb-io/drawdb /drawdb
#   git -C /drawdb checkout <revision>
#   docker buildx build -f deploy/drawdb/Dockerfile \
#     --build-arg DRAWDB_REVISION=$(git -C /drawdb rev-parse HEAD) \
#     --platform linux/amd64,linux/arm64 -t blockmill/drawdb:dev /drawdb
#
# The Vite build produces platform-independent files, so it runs once on the
# build machine; the runtime stage has no RUN step, so arm64 needs no QEMU.

ARG NODE_IMAGE=node:20-alpine
ARG NGINX_IMAGE=nginxinc/nginx-unprivileged:stable-alpine

FROM --platform=$BUILDPLATFORM ${NODE_IMAGE} AS build
WORKDIR /app
COPY package*.json ./
RUN npm ci
COPY . .
ENV NODE_OPTIONS="--max-old-space-size=4096"
RUN npm run build

FROM ${NGINX_IMAGE}
ARG DRAWDB_REVISION=unknown

LABEL org.opencontainers.image.title="drawdb" \
      org.opencontainers.image.description="drawDB, the free browser-based database diagram editor, served by an unprivileged nginx. Community image, not affiliated with the drawDB project." \
      org.opencontainers.image.licenses="AGPL-3.0-only" \
      org.opencontainers.image.source="https://github.com/drawdb-io/drawdb" \
      org.opencontainers.image.revision="${DRAWDB_REVISION}" \
      org.opencontainers.image.vendor="blockmill"

COPY --from=build /app/dist /usr/share/nginx/html
COPY LICENSE /licenses/LICENSE

# A single-page app: unknown paths fall back to index.html. Hashed assets are
# cached for a year; index.html is always revalidated.
COPY <<'EOF' /etc/nginx/conf.d/default.conf
server {
    listen 8080;
    server_name _;
    root /usr/share/nginx/html;

    location / {
        try_files $uri /index.html;
    }
    location /assets/ {
        add_header Cache-Control "public, max-age=31536000, immutable";
    }
    location = /index.html {
        add_header Cache-Control "no-cache";
    }
}
EOF

EXPOSE 8080
