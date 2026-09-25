<p align="center">
  <img src="https://raw.githubusercontent.com/TomDatalab/cube/e1415a660a3ff2deff7f0c896543193458ff0720/deploy/drawdb/logo.png" alt="blockmill/drawdb logo" width="150"/>
</p>

# drawDB: database diagrams in your browser

<p align="center">
  <a href="https://github.com/drawdb-io/drawdb"><b>drawDB source code</b></a> ·
  <a href="https://github.com/TomDatalab/cube/tree/main/deploy/drawdb">Image build files</a> ·
  <a href="https://hub.docker.com/r/blockmill/drawdb">Docker Hub</a>
</p>

**A ready-to-run image of [drawDB](https://github.com/drawdb-io/drawdb), the free,
open-source editor for entity-relationship diagrams.** Design tables and
relationships visually, then export SQL for your database, with no account and
no sign-up.

- **Tiny and fast.** The app is served as static files by nginx. The image is
  multi-arch (`linux/amd64`, `linux/arm64`) and starts instantly.
- **Hardened.** It runs as a non-root user (`nginx-unprivileged`) on port
  `8080`, with no build tools in the image.
- **Private by design.** Diagrams live in each user's browser storage. The
  container stores nothing and needs no database or volume.
- **Faithful to upstream.** It is built from an unmodified drawDB checkout, and
  the exact commit is recorded in the image labels.

> **Community image.** Not affiliated with or endorsed by the drawDB project.
> drawDB is licensed under the **GNU AGPL-3.0**; this image is distributed under
> the same license. The license text ships in the image at `/licenses/LICENSE`,
> and the corresponding source is linked below.

---

## Quick start

```bash
docker run -d --name drawdb -p 8080:8080 --restart unless-stopped blockmill/drawdb
```

Open **http://localhost:8080** and click *Try it* to open the editor.

### Docker Compose

```yaml
services:
  drawdb:
    image: blockmill/drawdb:latest
    ports:
      - "8080:8080"
    restart: unless-stopped
```

---

## What you can do with drawDB

- Draw tables, columns, indexes and relationships on a canvas.
- Import an existing schema from SQL or DBML, or start from a template.
- Export `CREATE TABLE` scripts for **MySQL, PostgreSQL, SQLite, MariaDB,
  SQL Server and Oracle**, or export the diagram as PNG, SVG, PDF, JSON,
  DBML or Mermaid.
- Add notes and subject areas, and use it in light or dark mode.

---

## Image details

| | |
|---|---|
| Tags | `latest`, `2026.09.24-723cb79` (drawDB commit `723cb79` from 2026-09-24) |
| Platforms | `linux/amd64`, `linux/arm64` |
| Base | `nginxinc/nginx-unprivileged:stable-alpine` |
| User | non-root (uid 101) |
| Port | `8080` |
| Routing | single-page-app fallback to `index.html`; hashed assets cached for a year, `index.html` always revalidated |
| License | AGPL-3.0, at `/licenses/LICENSE` |
| Source | [drawdb-io/drawdb](https://github.com/drawdb-io/drawdb); exact commit in label `org.opencontainers.image.revision` |

Tags are named `<drawDB commit date>-<commit>`, because drawDB does not publish
version numbers.

### Behind a reverse proxy

The app is fully static, so any proxy works. Point it at port `8080`:

```nginx
location / {
    proxy_pass http://drawdb:8080;
}
```

drawDB expects to be served at the root of a host, not under a sub-path.

---

## Building

The build files are in
[TomDatalab/cube, `deploy/drawdb/`](https://github.com/TomDatalab/cube/tree/main/deploy/drawdb):

```bash
git clone https://github.com/drawdb-io/drawdb /drawdb
docker buildx build -f deploy/drawdb/Dockerfile \
  --build-arg DRAWDB_REVISION=$(git -C /drawdb rev-parse HEAD) \
  --platform linux/amd64,linux/arm64 -t my/drawdb:dev /drawdb
```

The Vite build runs once on the build machine. The runtime stage only copies
files, so building for arm64 needs no emulation.
