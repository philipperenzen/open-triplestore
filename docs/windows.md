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
curl.exe http://localhost:7878/health     # -> {"status":"ok","version":"0.2.0"}
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
docker compose exec triplestore open-triplestore --data-dir /data --promote-super-admin <username>
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

A fully native build is possible but unsupported. `--features full` won't work because the
`saml` feature needs libxml2 + xmlsec1 (painful under MSVC), so you build **without `saml`**.
Every native build also links the **GEOS** C library (GeoSPARQL), which you supply via vcpkg.

### Prerequisites — none ship with Windows; install these first

| Tool | Why it's needed | Install |
|---|---|---|
| **Rust — MSVC toolchain** | compiles the backend | <https://rustup.rs/> (default host `x86_64-pc-windows-msvc`) |
| **VS Build Tools — "Desktop development with C++"** | MSVC compiler + linker + Windows SDK — compiles the bundled **RocksDB** store (`oxrocksdb-sys`) and GEOS | [Build Tools for Visual Studio](https://visualstudio.microsoft.com/downloads/#build-tools-for-visual-studio-2022) |
| **LLVM / Clang (`libclang`)** | `bindgen` generates FFI bindings for `oxrocksdb-sys` (and `geos-sys`) at build time | `winget install LLVM.LLVM`, then set `LIBCLANG_PATH` (step 2) |
| **vcpkg** | supplies the GEOS library | cloned in step 1 below (it fetches its own CMake + Ninja) |

> **This is a multi-GB toolchain** (the C++ Build Tools alone are several GB), and the first
> build **compiles RocksDB and GEOS from source**, so it is slow. If that's more than you
> want, use **Docker** (everything preinstalled) or **WSL2** (`apt-get install libgeos-dev
> libclang-dev cmake`) — both fully featured. Native is the hard path.

### 1. GEOS via [vcpkg](https://vcpkg.io)

```powershell
git clone https://github.com/microsoft/vcpkg
.\vcpkg\bootstrap-vcpkg.bat
.\vcpkg\vcpkg install geos:x64-windows
.\vcpkg\vcpkg list geos                 # note the version, e.g. "geos:x64-windows  3.13.0"
```

### 2. Environment — point the build at GEOS + libclang

This project pins `geos-sys` 2.0.x, which links dynamically from `GEOS_LIB_DIR` (with the
matching `GEOS_VERSION`) when both are set — so **no `pkg-config` is required**. In the
**same PowerShell** you'll build from:

```powershell
$vcpkg = "$PWD\vcpkg\installed\x64-windows"
$env:GEOS_LIB_DIR = "$vcpkg\lib"        # holds geos_c.lib (the import library)
$env:GEOS_VERSION = "3.13.0"            # must match `vcpkg list geos` above
$env:PATH = "$vcpkg\bin;$env:PATH"      # so geos_c.dll is found at run time
$env:LIBCLANG_PATH = "C:\Program Files\LLVM\bin"   # bindgen -> oxrocksdb-sys / geos-sys
```

To persist these across shells, set them under *System → Environment Variables* (or with
`setx`) instead of the per-session `$env:` assignments.

### 3. Build without `saml` and run

```powershell
cargo build --release --no-default-features `
  --features "rdf-12,owl2-rl,owl2-el,owl2-ql,owl2-dl,text-search,ldp,shex,swrl,asset-pdf,asset-exif,asset-media,asset-archive,asset-spreadsheet,asset-thumbnail,asset-clamav"
.\target\release\open-triplestore.exe --port 7878 --data-dir .\data
curl.exe http://localhost:7878/health    # -> {"status":"ok",...}
```

### Prebuilt GEOS — skip the C++ Build Tools

If you'd rather not compile GEOS, grab a prebuilt build and point the same `GEOS_LIB_DIR` /
`GEOS_VERSION` / `PATH` at it. You still need the Rust **MSVC** toolchain (for its linker):

- **conda-forge:** `conda install -c conda-forge geos` → import libs under `…\Library\lib`, DLLs under `…\Library\bin`.
- **OSGeo4W:** the GUI installer ships `geos_c.dll`, headers, and the import library.

(MSYS2's `mingw-w64-x86_64-geos` is GNU-ABI — pair it only with the `x86_64-pc-windows-gnu`
Rust toolchain, never the MSVC one.)

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
| `could not find native library 'geos_c'` / `GEOS_VERSION must be set` at build | `geos-sys` can't locate GEOS | Set **both** `GEOS_LIB_DIR` and `GEOS_VERSION` (Option 3, step 2) — or install `pkg-config` + GEOS |
| `STATUS_DLL_NOT_FOUND` (`0xc0000135`) / `geos_c.dll` missing at run time | GEOS DLL not on `PATH` | Add the vcpkg/conda `bin` dir to `PATH` (Option 3, step 2) |
| `Unable to find libclang` / `clang.dll` at build | `bindgen` (for `oxrocksdb-sys`) can't find LLVM | `winget install LLVM.LLVM`, set `LIBCLANG_PATH` to its `bin` (Option 3, step 2) |
| `link.exe`/`cl.exe` not found, or `error: linker not found` | MSVC C++ Build Tools missing | Install VS Build Tools with the "Desktop development with C++" workload (Option 3 prerequisites) |

See also: the main [README](../README.md), [administration guide](administration.md),
and [CONTRIBUTING](../CONTRIBUTING.md).
