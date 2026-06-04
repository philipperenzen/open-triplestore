# Running Open Triplestore on Windows

Open Triplestore runs on Windows three ways. Pick based on what you're doing:

| Path | Best for | Effort | Features |
|---|---|---|---|
| **Docker Desktop** | Just running it; production-like local stack | ⭐ easiest | All (`full`) |
| **WSL2 (Ubuntu)** | Developing the Rust backend / frontend | ⭐⭐ easy | All (`full`) |
| **Native MSVC** | Avoiding WSL/Docker entirely | ⭐⭐⭐ advanced / experimental | Partial (no `saml`) |

The native build depends on the **GEOS** C library (for GeoSPARQL) on every OS, and
on **libxmlsec1** for the optional `saml` feature. Those are trivial to install on
Debian/Ubuntu/macOS but awkward under MSVC — which is why **Docker or WSL2 are the
recommended routes on Windows**.

> **Line endings are already handled.** The repo ships a [`.gitattributes`](../.gitattributes)
> that pins shell scripts, `Makefile`, `Dockerfile`, Compose files and `.env` to LF,
> so a Windows checkout stays usable under WSL, Git Bash, and Docker. You don't need
> to change `core.autocrlf`.

---

## Option 1 — Docker Desktop (recommended)

This runs the exact same Linux image as macOS/Linux, so it's fully featured and the
commands match the main README.

### 1. Install

1. Install [Docker Desktop for Windows](https://docs.docker.com/desktop/install/windows-install/)
   and enable the **WSL2 backend** (the default on Windows 10/11). Docker Desktop
   installs and manages WSL2 for you if it isn't present.
2. Open **PowerShell** in the cloned repo.

### 2. Create `.env` (PowerShell — no OpenSSL needed)

Compose has no insecure defaults; it won't start until the secrets exist.

```powershell
Copy-Item .env.example .env
function New-Secret([int]$n) { -join ((1..$n) | ForEach-Object { '{0:x2}' -f (Get-Random -Maximum 256) }) }
Add-Content .env "JWT_SECRET=$(New-Secret 32)"
Add-Content .env "MINIO_ROOT_USER=$(New-Secret 8)"
Add-Content .env "MINIO_ROOT_PASSWORD=$(New-Secret 24)"
```

`Get-Random` is fine for generating these secrets locally. (Docker Compose strips the
`\r` that PowerShell adds, so a CRLF `.env` works — but if you edit it by hand, save
it as **LF** to be safe.)

### 3. Start and verify

```powershell
docker compose up -d
docker compose ps
curl.exe http://localhost:7878/health     # -> {"status":"ok","version":"0.1.0"}
```

Then open <http://localhost:7878/> and register the first user — it becomes
`super_admin`.

> Use **`curl.exe`**, not `curl`: in PowerShell, `curl` is an alias for
> `Invoke-WebRequest`, which has different flags and won't match the README examples.

### Useful commands

```powershell
docker compose logs -f triplestore     # tail logs
docker compose build --no-cache        # rebuild after code changes
docker compose down                    # stop, keep data
docker compose down -v                 # stop and WIPE all data (destructive)

# Promote a user without stopping the server
docker exec open-triplestore open-triplestore --data-dir /data --promote-super-admin <username>
```

### Standalone container (no MinIO)

```powershell
docker build -t open-triplestore .
docker run -p 7878:7878 -v triplestore_data:/data open-triplestore
```

The standalone container auto-generates a JWT secret into the `/data` volume, so no
`.env` is required for this mode.

---

## Option 2 — WSL2 (recommended for development)

WSL2 gives you a real Ubuntu environment, so the Linux build instructions work
unchanged and you get all features including `saml`.

### 1. Install WSL2 + Ubuntu

In an **Administrator PowerShell**:

```powershell
wsl --install -d Ubuntu
```

Reboot if prompted, then launch **Ubuntu** from the Start menu and create your UNIX
user. Everything below runs **inside the Ubuntu shell**.

### 2. Clone inside the Linux filesystem

Clone into your WSL home (`~`), **not** `/mnt/c/...`. Building under `/mnt/c` is slow
(cross-filesystem I/O) and mixes Windows tooling into a Linux build.

```bash
sudo apt-get update
sudo apt-get install -y git build-essential pkg-config cmake \
    libgeos-dev libclang-dev lld \
    libxml2-dev libxmlsec1-dev libssl-dev      # the last three are for the `saml` feature
git clone https://github.com/philipperenzen/open-triplestore.git
cd open-triplestore
```

> Already cloned on the Windows side? You can reach it at `/mnt/c/Users/<you>/...`,
> but for day-to-day work re-clone inside WSL for the speed and isolation above.

### 3. Install Rust (inside WSL)

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
```

### 4. Build, run, develop

```bash
# Backend
cargo build --release --features full
./target/release/open-triplestore --port 7878 --data-dir ./data

# Frontend (Node 20+; install via nvm or apt)
cd frontend
npm install
npm run dev      # http://localhost:5173, proxied to :7878
```

Open <http://localhost:7878/> (or the Vite dev server) in your **Windows browser** —
WSL2 forwards `localhost` automatically.

### Talking to host services

If a process running inside a Docker/WSL container needs to reach a server on the
Windows host (e.g. the optional service registry or an LLM gateway), use
`host.docker.internal` — already the default for `LD_REGISTRY_URL` in
[`docker-compose.yml`](../docker-compose.yml). It resolves on Docker Desktop for
Windows out of the box.

---

## Option 3 — Native Windows (MSVC) — experimental

A fully native build is possible but unsupported, and `--features full` won't work
because the `saml` feature needs libxml2 + xmlsec1, which are painful under MSVC.
Build **without `saml`** and provide GEOS yourself.

1. **Rust (MSVC toolchain).** Install from <https://rustup.rs/> and the
   *Build Tools for Visual Studio* (the "Desktop development with C++" workload) for
   the MSVC linker.

2. **GEOS via [vcpkg](https://vcpkg.io).**

   ```powershell
   git clone https://github.com/microsoft/vcpkg
   .\vcpkg\bootstrap-vcpkg.bat
   .\vcpkg\vcpkg install geos:x64-windows
   ```

   Point the `geos-sys` build at the vcpkg install (exact variables can vary by
   `geos-sys` version — consult the [`geos` crate docs](https://docs.rs/geos) if the
   build can't find the library):

   ```powershell
   $env:GEOS_LIB_DIR = "$PWD\vcpkg\installed\x64-windows\lib"
   $env:GEOS_VERSION = "3.13.0"   # match the version vcpkg installed
   # Make the runtime DLL discoverable at run time:
   $env:PATH = "$PWD\vcpkg\installed\x64-windows\bin;$env:PATH"
   ```

3. **Build a reduced feature set** (everything except `saml`):

   ```powershell
   cargo build --release --no-default-features `
     --features "rdf-12,owl2-rl,owl2-el,owl2-ql,owl2-dl,text-search,ldp,shex,swrl,asset-pdf,asset-exif,asset-media,asset-archive,asset-spreadsheet,asset-thumbnail,asset-clamav"
   .\target\release\open-triplestore.exe --port 7878 --data-dir .\data
   ```

If this is more friction than you want — and it usually is — use Docker or WSL2.

---

## PowerShell ⇄ bash cheat-sheet

The README's examples are written for bash. Here's how to translate the common
idioms when you're in **Windows PowerShell**. (In **WSL** and **Git Bash** the bash
examples work verbatim.)

| Task | bash | PowerShell |
|---|---|---|
| HTTP request | `curl …` | `curl.exe …` (avoid the `Invoke-WebRequest` alias) |
| Line continuation | trailing `\` | trailing backtick `` ` `` |
| Set an env var for one command | `VAR=x cmd` | `$env:VAR = 'x'; cmd` |
| Random hex secret | `openssl rand -hex 32` | `-join ((1..32) \| ForEach-Object { '{0:x2}' -f (Get-Random -Maximum 256) })` |
| Run the binary | `./target/release/open-triplestore` | `.\target\release\open-triplestore.exe` |
| Paths | `examples/foo.ttl` | `examples\foo.ttl` (forward slashes also work) |
| POST a file body | `--data-binary @shapes.ttl` | `curl.exe --data-binary "@shapes.ttl"` (same flag; `curl.exe`) |

### Example: a multi-line `curl` from the README, in PowerShell

```bash
# README (bash)
curl -X POST http://localhost:7878/sparql \
     -H 'Content-Type: application/sparql-query' \
     -d 'SELECT ?name WHERE { ?s <http://xmlns.com/foaf/0.1/name> ?name }'
```

```powershell
# PowerShell — curl.exe + backtick continuation
curl.exe -X POST http://localhost:7878/sparql `
     -H "Content-Type: application/sparql-query" `
     -d "SELECT ?name WHERE { ?s <http://xmlns.com/foaf/0.1/name> ?name }"
```

---

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| `Invoke-WebRequest : Cannot bind parameter 'Headers'` | You used `curl` (the alias) with `-H` | Use `curl.exe` |
| `required variable JWT_SECRET is missing a value` | No `.env` for `docker compose` | Create `.env` (see Option 1, step 2) |
| `/bin/bash^M: bad interpreter` in WSL | A script got CRLF endings | Pull latest (the repo's `.gitattributes` pins scripts to LF); or `dos2unix <file>` |
| `make: *** … Error 127` under Git Bash | `make` not installed, or `docker-compose` (v1) missing | The `Makefile` targets assume a Unix shell + Docker Compose v2; run them in WSL |
| Docker build can't reach `host.docker.internal` | Not on Docker Desktop | It's Docker-Desktop-only; on plain Linux use the host IP |

See also: the main [README](../README.md), [administration guide](administration.md),
and [CONTRIBUTING](../CONTRIBUTING.md).
